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
- `obj.attr`, `obj[key]`, `f(args, kw=arg)` on worker handles — for JS calls,
  kwargs become a trailing options object (`cow.say(text = "moo")`); JS
  methods are `this`-bound at the boundary; promises are awaited; ESM default
  exports are unwrapped
- builtins: `print(...)`, `len(x)` (`__len__` for py refs, `.length` for js),
  `range(n)` / `range(a, b)`, `str(x)`
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
Nine ops: `import`, `getattr`, `call`, `index`, `binop`, `iter`, `next`,
`str`, `release`. Every reply
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
Writing it surfaced the real friction list, in rough priority order:

- **no item/attr assignment on refs** — `df["days"] = ...` doesn't parse;
  `.assign(days=...)` works but a `setitem`/`setattr` op is the obvious next op
- **no `new`** — JS class instances (`new Chalk({level: 3})`) can't be
  constructed, which is why the script hand-rolls ANSI codes
- **no `\x1b`/`\u{...}` string escapes** — `py.chr(27)` is the workaround
- **no argv** — scripts can't take arguments yet
- **no methods on data strings** — slicing/`split`/`ljust` need a worker
  (`py.format(x, "<16")`) or pandas `.str` methods
- **NaN poisons data marshaling** — one NaN in `to_dict("records")` turns the
  whole result into a ref instead of data (`json.dumps(allow_nan=False)` is
  strict by design); `.fillna()` first, but the failure is subtle

One friction got fixed on the spot: `else` used to be required on the same
line as `}` — the parser now looks ahead across newlines.

## Deliberately punted (so far)

- synchronous JS callbacks (`[1,2].map(f)` gets promises — Node can't block;
  Python callbacks are fully synchronous)
- async JS iterators (`for await` protocol — sync iterables only for now)
- handle release before exit (handles free when the worker dies)
- cross-worker refs, streaming/async beyond awaited promises, big-data (Arrow)
