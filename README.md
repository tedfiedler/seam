# seam

A tiny scripting language that borrows its batteries. Foreign packages run in
persistent worker subprocesses — Python *and* Node — while seam holds handles
to their objects and marshals JSON across the boundary. Foreign values never
enter the seam process — that one rule buys crash isolation, no GC
entanglement, and free use of two package ecosystems at once.

```
use py "pandas" as pd
use js "cowsay" as cow

let df = pd.read_csv("examples/sales.csv")
let total = df["amount"].sum()
print(cow.say(text = "sales: " + total))
```
```
 _______________
< sales: 5437.0 >
 ---------------
        \   ^__^
         \  (oo)\_______
            (__)\       )\/\
                ||----w |
```

**New here? Start with the [guide](docs/GUIDE.md)** — setup, the full
language reference, and worked examples.

## Install

```sh
brew install tedfiedler/tap/seam          # macOS / Linux, via Homebrew
cargo install --git https://github.com/tedfiedler/seam   # from source
```

Or grab a prebuilt binary from the
[releases](https://github.com/tedfiedler/seam/releases) (macOS
arm64/x86_64, Linux x86_64/arm64). `use py` wants `python3` on PATH and
`use js` wants `node` — neither spawns until a script asks. Windows is
untested (workers assume unix-y paths).

## Run

```sh
cargo run -q                            # REPL (multi-line aware)
cargo run -q -- examples/fib.seam       # run a script
cargo run -q -- examples/report.seam    # pandas + control flow
cargo run -q -- examples/polyglot.seam  # pandas + cowsay, one process
```

Python resolves via `.venv/bin/python3` in the cwd (else `python3`); Node
resolves `node_modules` from the cwd. So `pip install` and `npm install` *are*
the package manager. This repo's `.venv` has pandas; `npm install` fetches
cowsay and chalk for the demos.

## Language surface (weekend-2 scope)

- `use py "module" as name` / `use js "module" as name` — each language's
  worker spawns on first use; refs remember which worker owns them
- `let x = expr` to define, `x = expr` to assign (assignments see enclosing scopes)
- `fn name(a, b) { ... }` with `return` — real closures, recursion works
- `if (c) { } else if (c) { } else { }`, `while (c) { }`, `for (x in xs) { }`,
  `break` / `continue` — `for` iterates arrays, object keys, strings, and
  **worker refs directly**: Series values, DataFrame columns, dicts, Sets,
  generators — streamed lazily one `next` per item, so `break` works even on
  infinite generators (`for (i in it.count(100)) { ... break }`)
- operators: `+ - * / %`, `== != < <= > >=`, `and or not` (short-circuit,
  Python-style value semantics); only `nil` and `false` are falsy
- **operators delegate to workers**: with a ref on either side, the owning
  worker applies its own semantics — `df["amount"] > 1000` is a boolean
  Series, `df[mask]` filters, arithmetic broadcasts, `-series` negates, and
  `and`/`or` on refs become elementwise `&`/`|` (pandas mask combination):
  `df[(df["amount"] > 500) and (df["amount"] < 1500)]`
- strings, numbers, `true/false/nil`, `[arrays]`, `{objects}` (JSON-shaped data)
- `obj.attr`, `obj[key]`, `f(args, kw=arg)` on worker handles — plus
  **assignment**: `df["col"] = series` and `obj.attr = v`, and **`new`** for
  JS classes (`new h.Accumulator(10)`; on Python classes `new` is just a
  call) — for JS calls,
  kwargs become a trailing options object (`cow.say(text = "moo")`); JS
  methods are `this`-bound at the boundary; promises are awaited; ESM default
  exports are unwrapped
- builtins: `print(...)`, `len(x)` (`__len__` for py refs, `.length` for js),
  `range(n)` / `range(a, b)`, `str(x)`, `split(s, sep)`, `join(sep, xs)`,
  `slice(x, a, b)` (negative indices ok); scripts get `argv`
- string escapes: `\n \t \" \\ \x1b \u{1F44B}`
- **callbacks**: seam functions cross the boundary — `py.map(double, xs)`,
  `py.sorted(xs, key=by_len)`, `df["amount"].apply(f)` all work, callbacks
  close over seam state, and a callback may re-enter the worker that's
  waiting on it; in JS a seam function arrives as a promise-returning
  function (Node can't block), so async JS code awaits it naturally
- `#` comments; newlines end statements (parens/brackets may span lines);
  `else` may follow the `}` on the same line or the next
- errors carry line numbers; Python exceptions carry their full traceback

## How it works

`src/worker.rs` spawns `worker/worker.py` and/or `worker/worker.js` (both
embedded in the binary) and speaks newline-delimited JSON over stdin/stdout.
Thirteen ops: `import`, `getattr`, `setattr`, `call`, `new`, `index`,
`setitem`, `binop`, `iter`, `next`, `str`, `release`, `stats`.

Handles are garbage-collected: every ref carries a shared guard, and when
the last seam copy drops, its id lands in a per-worker graveyard that's
drained — one batched `release` — at the next statement boundary. Only
objects seam still holds stay alive in worker heaps; `stats()` shows the
live count per worker if you want to watch. Every reply
value is either `{"$":"data","v":...}` (JSON-shaped → copied into seam) or
`{"$":"ref","id":n,"repr":"..."}` (lives in that worker's heap; seam holds the
handle and routes later ops back to the owning worker). Passing one worker's
ref to the other is an error — workers don't share heaps; data is the common
currency.

Seam functions cross as `{"$":"fn","id":k}`. When a worker invokes one it
sends `{"cb":true,"fn":k,"args":[...]}` upstream and waits; the host runs the
function and replies `{"cbr":true,...}`. Both sides are re-entrant — a worker
waiting on a callback still serves nested requests, and the host runs nested
callbacks — with strictly LIFO nesting, so one synchronous pump on each side
suffices. Workers redirect their real stdout to stderr so stray prints can't
corrupt the protocol. Foreign exceptions come back carrying the full
traceback/stack; the REPL survives them. A boundary crossing costs ~30µs.

## Field notes from the first real script

`scripts/repos.seam` is a working repo-health dashboard: GitHub API →
pandas (freshness, stars, issues) → ANSI-colored table → cowsay verdict.
Try it on anyone: `seam scripts/repos.seam torvalds`.

Writing it surfaced a friction list that drove the next release: no
item/attr assignment on refs, no `new`, no `\x1b` escapes, no argv, no
string helpers — **all fixed in 0.7.0** (plus, en route, `else` on the
line after `}`). What remains true and deliberate:

- **NaN is not data** — one NaN in `to_dict("records")` turns the whole
  result into a ref instead of data (silently coercing NaN→null would
  lie); `.fillna()` first
- **data strings have no methods** — use `split`/`join`/`slice` builtins,
  `py.format`, or pandas `.str.*`

## Deliberately punted (so far)

- synchronous JS callbacks (`[1,2].map(f)` gets promises — Node can't block;
  Python callbacks are fully synchronous)
- async JS iterators (`for await` protocol — sync iterables only for now)
- releasing callback registrations (seam fns handed to workers stay
  registered until exit — worker-side finalizers would be needed)
- cross-worker refs, streaming/async beyond awaited promises, big-data (Arrow)
