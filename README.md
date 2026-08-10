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
cargo run -q                        # REPL
cargo run -q < examples/demo.seam   # scripted
```

Uses `.venv/bin/python3` if the cwd has one, else `python3` — so `pip install`
into a venv (or `node_modules`, eventually) is the whole package-management
story. This repo's `.venv` has pandas.

## Language surface (weekend-1 scope)

- `use py "module" as name` — spawns the Python worker on first use
- `let x = expr`, arithmetic `+ - * /` (`+` concatenates when a string is involved)
- strings, numbers, `true/false/nil`, `[arrays]`, `{objects}` (JSON-shaped data)
- `obj.attr`, `obj[key]`, `f(args, kw=arg)` on Python handles
- `print(x)` — full `str()` of a handle (the REPL shows only a one-line repr)
- `#` comments

## How it works

`src/worker.rs` spawns `worker/worker.py` (embedded in the binary) and speaks
newline-delimited JSON over stdin/stdout. Six ops: `import`, `getattr`, `call`,
`index`, `str`, `release`. Every reply value is either
`{"$":"data","v":...}` (JSON-able → copied into seam) or
`{"$":"ref","id":n,"repr":"..."}` (lives in the worker's heap; seam holds the
handle). The worker redirects Python's real stdout to stderr so stray prints
can't corrupt the protocol. Python exceptions come back as errors carrying the
full traceback; the REPL survives them. A boundary crossing costs ~30µs.

## Deliberately punted (v1)

- callbacks into seam (passing seam functions to Python) — needs a reverse op
- handle release before exit (handles free when the worker dies)
- a Node worker, cross-worker refs, async, big-data paths (Arrow)
- control flow / functions in seam itself — this is the protocol weekend;
  the interpreter weekend comes next
