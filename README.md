# seam

A tiny scripting language that borrows its batteries. Foreign packages run in
persistent worker subprocesses (Python today; Node next); seam holds handles to
their objects and marshals JSON across the boundary. Foreign values never enter
the seam process — that one rule buys crash isolation, no GC entanglement, and
free use of an entire package ecosystem.

```
» use py "pandas" as pd
» let df = pd.read_csv("examples/sales.csv")
» df["amount"].sum()
5437.0
» df.groupby("region")["amount"].sum().to_dict()
{"east":2270.75,"west":3166.25}
```

## Run

```sh
cargo run -q                          # REPL (multi-line aware)
cargo run -q -- examples/fib.seam     # run a script
cargo run -q -- examples/report.seam  # pandas + control flow together
```

Uses `.venv/bin/python3` if the cwd has one, else `python3` — so `pip install`
into a venv (or `node_modules`, eventually) is the whole package-management
story. This repo's `.venv` has pandas.

## Language surface (weekend-2 scope)

- `use py "module" as name` — spawns the Python worker on first use
- `let x = expr` to define, `x = expr` to assign (assignments see enclosing scopes)
- `fn name(a, b) { ... }` with `return` — real closures, recursion works
- `if (c) { } else if (c) { } else { }`, `while (c) { }`, `for (x in xs) { }`,
  `break` / `continue` — `for` iterates arrays, object keys, and strings
- operators: `+ - * / %`, `== != < <= > >=`, `and or not` (short-circuit,
  Python-style value semantics); only `nil` and `false` are falsy
- strings, numbers, `true/false/nil`, `[arrays]`, `{objects}` (JSON-shaped data)
- `obj.attr`, `obj[key]`, `f(args, kw=arg)` on Python handles
- builtins: `print(...)`, `len(x)` (works on Python refs via `__len__`),
  `range(n)` / `range(a, b)`, `str(x)`
- `#` comments; newlines end statements (parens/brackets may span lines);
  `else` goes on the same line as the closing `}`
- errors carry line numbers; Python exceptions carry their full traceback

## How it works

`src/worker.rs` spawns `worker/worker.py` (embedded in the binary) and speaks
newline-delimited JSON over stdin/stdout. Six ops: `import`, `getattr`, `call`,
`index`, `str`, `release`. Every reply value is either
`{"$":"data","v":...}` (JSON-able → copied into seam) or
`{"$":"ref","id":n,"repr":"..."}` (lives in the worker's heap; seam holds the
handle). The worker redirects Python's real stdout to stderr so stray prints
can't corrupt the protocol. Python exceptions come back as errors carrying the
full traceback; the REPL survives them. A boundary crossing costs ~30µs.

## Deliberately punted (so far)

- callbacks into seam (passing seam functions to Python) — needs a reverse op
- iterating a Python ref directly (`.tolist()` / `.to_dict()` it first) and
  operators on refs (`df["x"] == "east"` — needs an operator-protocol op)
- handle release before exit (handles free when the worker dies)
- a Node worker, cross-worker refs, async, big-data paths (Arrow)
