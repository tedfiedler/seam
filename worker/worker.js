// seam node worker — NDJSON request/response on stdout; console goes to stderr
const readline = require('readline');
const util = require('util');
const path = require('path');
const { createRequire } = require('module');
const { pathToFileURL } = require('url');

// Resolve bare specifiers against the script's cwd, so node_modules just works.
const cwdRequire = createRequire(path.join(process.cwd(), '__seam__.js'));

// Stray console output from libraries must not corrupt the protocol.
const proto = process.stdout;
console.log = (...a) => console.error(...a);
console.info = (...a) => console.error(...a);
console.warn = (...a) => console.error(...a);

const objs = new Map();
let nextId = 0;

// Data = JSON-shaped values only. Class instances, functions, Maps, NaN,
// BigInt etc. stay here as refs — JSON.stringify would silently mangle them.
function isData(v) {
  if (v === null || typeof v === 'boolean' || typeof v === 'string') return true;
  if (typeof v === 'number') return Number.isFinite(v);
  if (Array.isArray(v)) return v.every(isData);
  if (typeof v === 'object') {
    const p = Object.getPrototypeOf(v);
    return (p === Object.prototype || p === null) && Object.values(v).every(isData);
  }
  return false;
}

function toWire(v) {
  if (v === undefined) return { $: 'data', v: null };
  if (isData(v)) return { $: 'data', v };
  nextId += 1;
  objs.set(nextId, v);
  const repr = util.inspect(v, { depth: 1 }).split('\n')[0].slice(0, 80);
  return { $: 'ref', id: nextId, repr };
}

function fromWire(w) {
  if (Array.isArray(w)) return w.map(fromWire);
  if (w && typeof w === 'object') {
    if (w.$ === 'ref') return objs.get(w.id);
    if (w.$ === 'fn') return makeCallback(w.id);
    const o = {};
    for (const [k, v] of Object.entries(w)) o[k] = fromWire(v);
    return o;
  }
  return w;
}

// Seam functions arrive as handles. Node can't block on stdin, so invoking
// one returns a promise; replies resolve LIFO (nesting is strict).
const pendingCbs = [];

function makeCallback(fnId) {
  return (...args) =>
    new Promise((resolve, reject) => {
      pendingCbs.push({ resolve, reject });
      proto.write(JSON.stringify({ cb: true, fn: fnId, args: args.map(toWire) }) + '\n');
    });
}

// ESM namespaces hide the useful thing behind .default — unwrap it.
function preferDefault(mod) {
  if (mod && (mod[Symbol.toStringTag] === 'Module' || mod.__esModule) && 'default' in mod) {
    return mod.default;
  }
  return mod;
}

async function importModule(name) {
  try {
    return preferDefault(cwdRequire(name));
  } catch (e) {
    let spec = name;
    try { spec = pathToFileURL(cwdRequire.resolve(name)).href; } catch {}
    return preferDefault(await import(spec));
  }
}

async function handle(req) {
  switch (req.op) {
    case 'import':
      return await importModule(req.name);
    case 'getattr': {
      const obj = fromWire(req.obj);
      const res = obj[req.name];
      return typeof res === 'function' ? res.bind(obj) : res;
    }
    case 'index':
      return fromWire(req.obj)[fromWire(req.key)];
    case 'call': {
      const fn = fromWire(req.obj);
      if (typeof fn !== 'function') throw new TypeError('value is not a function');
      const args = (req.args || []).map(fromWire);
      const kw = req.kwargs || {};
      if (Object.keys(kw).length > 0) args.push(fromWire(kw)); // kwargs -> options object
      return await fn(...args);
    }
    case 'str':
      return util.inspect(fromWire(req.obj), { depth: 2 });
    case 'release':
      objs.delete(req.ref);
      return null;
    default:
      throw new Error('unknown op: ' + req.op);
  }
}

const rl = readline.createInterface({ input: process.stdin, terminal: false });
rl.on('line', async (line) => {
  if (!line.trim()) return;
  const req = JSON.parse(line);
  if (req.cbr) {
    const p = pendingCbs.pop();
    if (p) {
      req.ok ? p.resolve(fromWire(req.value)) : p.reject(new Error('seam callback failed: ' + req.error));
    }
    return;
  }
  let res;
  try {
    res = { id: req.id, ok: true, value: toWire(await handle(req)) };
  } catch (e) {
    res = {
      id: req.id,
      ok: false,
      error: e instanceof Error ? `${e.name}: ${e.message}` : String(e),
      trace: (e instanceof Error && e.stack) || '',
    };
  }
  proto.write(JSON.stringify(res) + '\n');
});
