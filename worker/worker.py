import sys, json, importlib, traceback

# Stray prints from imported libraries must not corrupt the protocol stream.
proto = sys.stdout
sys.stdout = sys.stderr

objs = {}
next_id = 0


def send(msg):
    proto.write(json.dumps(msg) + "\n")
    proto.flush()


def read_msg():
    while True:
        line = sys.stdin.readline()
        if not line:
            sys.exit(0)
        if line.strip():
            return json.loads(line)


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
        t = w.get("$")
        if t == "ref":
            return objs[w["id"]]
        if t == "fn":
            return make_callback(w["id"])
        return {k: from_wire(v) for k, v in w.items()}
    if isinstance(w, list):
        return [from_wire(x) for x in w]
    return w


def make_callback(fn_id):
    def cb(*args, **kwargs):
        if kwargs:
            raise TypeError("seam functions don't take keyword arguments")
        send({"cb": True, "fn": fn_id, "args": [to_wire(a) for a in args]})
        while True:
            msg = read_msg()
            if msg.get("cbr"):
                if msg.get("ok"):
                    return from_wire(msg["value"])
                raise RuntimeError("seam callback failed: " + msg.get("error", "unknown"))
            # the host re-entered us while evaluating the callback
            handle_request(msg)

    return cb


def handle_request(req):
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
    send(out)


while True:
    handle_request(read_msg())
