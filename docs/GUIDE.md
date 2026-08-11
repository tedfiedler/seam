# The seam guide

seam is a tiny scripting language that borrows its batteries. Python and
Node packages run in worker subprocesses; seam holds handles to their
objects and moves JSON-shaped data across the boundary. You write one
script; pandas and npm both answer.

```
┌──────────────┐  NDJSON pipes   ┌─────────────────┐
│  seam (Rust) │◄───────────────►│ python3 worker   │  .venv + pip
│  your script │                 └─────────────────┘
│  runs here   │◄───────────────►┌─────────────────┐
└──────────────┘                 │ node worker      │  node_modules + npm
                                 └─────────────────┘
```

The one rule everything follows: **foreign values never enter the seam
process.** JSON-shaped values (numbers, strings, bools, nil, arrays,
objects) are *data* — they copy across and become seam values. Everything
else (a DataFrame, a JS class instance, a generator) stays in its worker's
heap and seam holds a *ref* — a numbered handle. Operations on a ref
(attributes, calls, operators, iteration) route back to the worker that
owns it.

---

## Setup

You need the Rust toolchain, plus `python3` and/or `node` on your PATH —
each worker spawns only when a script first uses its language.

```sh
git clone https://github.com/tedfiedler/seam
cd seam
cargo build --release                # ./target/release/seam
cargo install --path .               # optional: puts `seam` on PATH
```

### Packages are pip's and npm's problem — that's the design

Workers resolve packages from the directory you run seam in:

- **Python**: if `./.venv/bin/python3` exists, the worker uses it;
  otherwise the system `python3`. So `python3 -m venv .venv &&
  .venv/bin/pip install pandas` and you're done.
- **Node**: bare imports resolve against `./node_modules`, so
  `npm install cowsay` and you're done. Both CommonJS and ESM packages
  work; ESM default exports are unwrapped for you.

To run this repo's demos after cloning:

```sh
python3 -m venv .venv && .venv/bin/pip install pandas
npm install                          # cowsay + chalk, pinned in package.json
```

### Running

```sh
seam                        # REPL — multi-line aware, :q or ctrl-d quits
seam script.seam            # run a file (expression results not echoed)
seam < script.seam          # pipe mode — echoes results like the REPL
```

In the REPL, an unfinished block (`fn`, `if`, an open `[`) switches the
prompt to `…` until your braces balance.

---

## Language reference

### Values

| kind | examples | notes |
|------|----------|-------|
| data | `42`, `1.5`, `"hi"`, `true`, `nil`, `[1, 2]`, `{a: 1}` | JSON-shaped, lives in seam |
| ref  | `<py:3 DataFrame…>`, `<js:1 [Function]>` | lives in a worker, seam holds the handle |
| fn   | `<fn double(x)>` | seam function; can cross into workers as a callback |

Object literal keys are bare identifiers or strings: `{name: "ted",
"full name": "..."}`. Arrays and objects hold data only — refs can't be
placed inside literals (pass them separately).

String escapes: `\n \t \" \\`, plus `\x1b` (two hex digits) and
`\u{1F44B}` (a codepoint) — ANSI colors and emoji without workarounds.

### Variables and scope

```
let x = 5        # define in the current scope
x = 6            # assign — walks outward through enclosing scopes
df["col"] = s    # item assignment on a worker ref (worker's setitem)
obj.attr = v     # attribute assignment on a worker ref
```

Assignment without `let` requires the variable to exist somewhere up the
chain; otherwise it's an error. Blocks (`{ }` after if/while/for/fn) open
a new scope. Closures capture their environment by reference — inner
functions can mutate outer variables (see the counter example below).

### Operators

Precedence, loosest to tightest:

```
or  →  and  →  not  →  == != < <= > >=  →  + -  →  * / %  →  unary -  →  calls/attrs/index
```

- `+` concatenates when either side is a string; numbers otherwise.
- Comparisons don't chain (`a < b < c` is a parse error).
- `and`/`or` short-circuit and return the deciding *value* (Python-style):
  `x or "default"`.
- Truthiness: only `nil` and `false` are falsy. `0` and `""` are truthy.
- `not x` gives a real bool.

**Refs make operators delegate.** If either operand is a ref, the whole
operation is shipped to the owning worker, which applies its native
semantics:

```
df["amount"] > 1000            # boolean Series (pandas semantics)
df[df["region"] == "east"]     # boolean indexing
df["amount"] * 1.08            # broadcasts
-df["amount"]                  # negation delegates too
```

`and`/`or` with a ref operand become **elementwise** `&`/`|` — exactly
what combining pandas masks means:

```
df[(df["amount"] > 500) and (df["amount"] < 1500)]
```

Mixing refs from different workers in one operator is an error — workers
don't share heaps; data is the common currency.

### Control flow

```
if (cond) { ... }
else if (other) { ... }        # else may share the } line or take its own
else { ... }

while (cond) { ... }

for (x in xs) { ... }          # arrays → elements, objects → keys,
                               # strings → characters, refs → see below

break    continue
```

Conditions take parentheses; bodies take braces, even one-liners.
Newlines end statements; inside `(` and `[` you may break lines freely.

**`for` over a ref streams lazily** — one protocol round-trip per element,
using the worker's own iteration (Series → values, DataFrame → column
names, dict → keys, JS Set/Map/generators via `Symbol.iterator`). Because
nothing is prefetched, `break` works even on infinite generators:

```
use py "itertools" as it
for (i in it.count(100)) {
  if (i >= 103) { break }      # exits cleanly; the generator just stops being asked
}
```

### Functions

```
fn greet(name) {
  return "hello " + name       # bare `return` returns nil; so does falling off the end
}

fn make_counter() {
  let n = 0
  fn inc() {
    n = n + 1        # closure: captures and mutates n
    return n
  }
  return inc
}
```

Seam functions take positional args only (arity checked). Recursion works.
Functions are values — pass them, return them, and hand them to workers:

### Callbacks — seam functions inside workers

```
fn double(x) { return x * 2 }
py.list(py.map(double, [1, 2, 3]))          # [2,4,6]
py.sorted(words, key=by_len)                # kwargs carry functions too
df["amount"].apply(classify)                # pandas apply
```

Python invokes seam callbacks **synchronously**, and a callback may
re-enter the worker that's waiting on it (`fn f(x) { return py.abs(x) }`
inside `py.map` is fine). Callbacks close over seam state like any
closure. Errors inside a callback propagate out as the worker's exception.

In JS, a seam function arrives as a **promise-returning** function (Node
can't block), so async JS awaits it naturally — but sync APIs like
`[1,2].map(f)` would get promises. Design JS-side helpers as async.

### Foreign modules

```
use py "pandas" as pd          # spawns the python worker on first use
use py "builtins" as py        # python's builtins are a module too — py.round, py.chr, py.format
use js "cowsay" as cow         # npm package
use js "./helpers.js" as h     # local file, relative to the cwd
```

Attribute access, indexing, and calls on refs do what you'd expect:

```
pd.read_csv("sales.csv")               # call, returns a ref
df["amount"]                           # index
df.groupby("region")["amount"].sum()   # chains freely (each hop ~30µs)
pd.read_csv("s.csv", nrows=2)          # kwargs: real kwargs in python...
cow.say(text = "moo")                  # ...a trailing options object in JS
```

JS methods are `this`-bound at the boundary, and promises returned by JS
calls are awaited before crossing — `await`-free async consumption.

**Constructing instances**: `new` builds JS class instances
(`Reflect.construct` underneath); on Python classes it's identical to a
plain call, so use whichever reads better:

```
let acc = new h.Accumulator(10)     # JS class
acc.total = 0                       # setattr on the instance
new pd.Timestamp("2026-01-01")      # same as pd.Timestamp(...)
```

`new` binds to one call — to chain off a fresh instance, parenthesize:
`(new h.Accumulator(5)).add(1)`.

### Builtins

| builtin | behavior |
|---------|----------|
| `print(a, b, ...)` | joins with spaces; refs print their full `str()`/`inspect()` |
| `len(x)` | strings, arrays, objects; py refs via `__len__`; js refs via `.length` |
| `range(n)` / `range(a, b)` | array of integers, half-open |
| `str(x)` | stringify anything, workers consulted for refs |
| `split(s, sep)` | `split("a-b", "-")` → `["a","b"]` |
| `join(sep, xs)` | `join("/", parts)` — numbers stringify |
| `slice(x, a, b?)` | strings (by char) and arrays; negatives count from the end |

Scripts also get **`argv`** — the arguments after the script path
(`seam repos.seam torvalds` → `argv` is `["torvalds"]`; empty in the REPL).

### Errors

Parse errors carry line numbers. Worker exceptions arrive with their full
Python traceback or JS stack, and the REPL survives them — the worker
lives on, your session keeps its state.

### Sharp edges (current)

- **Data strings have no methods** — `"abc".upper` is an error, because
  data lives in seam, not Python. Use the `split`/`join`/`slice`
  builtins, `py.format(x, "<16")`, or pandas `.str.*`.
- **Seam data is immutable** — `xs[0] = 5` on a seam array is an error;
  item/attr assignment is for worker refs. Build new arrays instead.
- **NaN is not data.** A NaN anywhere in a structure makes the whole
  marshal a ref instead of data (deliberately — silent NaN→null would
  lie). `.fillna(...)` before `.to_dict()`.

---

## Examples

All runnable from the repo root; longer versions live in `examples/`.

**Hello, two ecosystems** (`examples/polyglot.seam`)

```
use py "pandas" as pd
use js "cowsay" as cow

let df = pd.read_csv("examples/sales.csv")
print(cow.say(text = "sales: " + df["amount"].sum()))
```

**Filtering and aggregating** (`examples/operators.seam`)

```
let east = df[df["region"] == "east"]
let mid  = df[(df["amount"] > 500) and (df["amount"] < 1500)]
print("with tax:", (df["amount"] * 1.08).round(2).tolist())
```

**Closures** (`examples/fib.seam`)

```
let c = make_counter()   # see definition above
c()
c()
print(c())               # 3
```

**Callbacks** (`examples/callbacks.seam`)

```
let calls = 0
fn tick(x) {
  calls = calls + 1
  return x * 10
}
py.list(py.map(tick, [1, 2, 3]))     # [10,20,30] — and calls is now 3
```

**Streaming iteration** (`examples/iterate.seam`)

```
for (amt in df["amount"]) { sum = sum + amt }     # Series, lazily
for (p in h.primes) { print(p) }                  # a JS Set
```

**A real program** — `scripts/repos.seam`, a GitHub repo-health dashboard:
fetches your public repos, classifies freshness with Timestamp math,
prints an ANSI-colored table, and lets cowsay announce the most-starred
repo. Thirty lines, both workers, most of the language. Read it last:
it's the best picture of what seam code actually looks like.

---

*Protocol details, architecture, and the roadmap live in the
[README](../README.md).*
