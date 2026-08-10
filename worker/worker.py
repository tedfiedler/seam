import sys, json, importlib, traceback

# Stray prints from imported libraries must not corrupt the protocol stream.
proto = sys.stdout
sys.stdout = sys.stderr

objs = {}
next_id = 0


def to_wire(v):
    global next_id
    if hasattr(v, "item"):  # numpy-style scalars -> plain numbers
        try:
            v = v.item()
        except Exception:
            pass
    try:
        json.dumps(v, allow_nan=False)
        return {"$": "data", "v": v}
    except (TypeError, ValueError, OverflowError):
        next_id += 1
        objs[next_id] = v
        return {"$": "ref", "id": next_id, "repr": repr(v).split("\n")[0][:80]}


def from_wire(w):
    if isinstance(w, dict):
        if w.get("$") == "ref":
            return objs[w["id"]]
        return {k: from_wire(v) for k, v in w.items()}
    if isinstance(w, list):
        return [from_wire(x) for x in w]
    return w


for line in sys.stdin:
    if not line.strip():
        continue
    req = json.loads(line)
    try:
        op = req["op"]
        if op == "import":
            res = importlib.import_module(req["name"])
        elif op == "getattr":
            res = getattr(from_wire(req["obj"]), req["name"])
        elif op == "index":
            res = from_wire(req["obj"])[from_wire(req["key"])]
        elif op == "call":
            res = from_wire(req["obj"])(
                *[from_wire(a) for a in req.get("args", [])],
                **{k: from_wire(v) for k, v in req.get("kwargs", {}).items()},
            )
        elif op == "str":
            res = str(from_wire(req["obj"]))
        elif op == "release":
            objs.pop(req["ref"], None)
            res = None
        else:
            raise ValueError(f"unknown op: {op}")
        out = {"id": req["id"], "ok": True, "value": to_wire(res)}
    except Exception as e:
        out = {
            "id": req["id"],
            "ok": False,
            "error": f"{type(e).__name__}: {e}",
            "trace": traceback.format_exc(),
        }
    proto.write(json.dumps(out) + "\n")
    proto.flush()
