// A Glossa module in plain Node.js (18 or newer), with no SDK: the whole
// module protocol is the sixty lines below. It shows what a module in any
// language has to do over stdio:
//
// * read one JSON object per line from stdin: {"id": 7, "method": "translate", "params": {...}}
// * answer with one line on stdout: {"id": 7, "result": {...}} or {"id": 7, "error": {"kind", "message"}}
// * while handling a request, call back into Glossa with a string id:
//   {"id": "h1", "method": "log", "params": {...}}, then read {"id": "h1", "result": ...}
//
// translate reverses the order of the words; detect claims English.

import { createInterface } from "node:readline";

// Keep console.log off the protocol stream: Glossa logs stderr.
console.log = console.error;

class ModuleError extends Error {
  constructor(kind, message) {
    super(message);
    this.kind = kind;
  }
}

const lines = createInterface({ input: process.stdin, crlfDelay: Infinity })[Symbol.asyncIterator]();

async function nextFrame() {
  for (;;) {
    const { value, done } = await lines.next();
    if (done) return null;
    try {
      const frame = JSON.parse(value);
      if (frame && typeof frame === "object") return frame;
    } catch {
      // not JSON: skip it
    }
  }
}

function send(frame) {
  process.stdout.write(JSON.stringify(frame) + "\n");
}

let hostCalls = 0;
async function host(method, params) {
  const id = `h${++hostCalls}`;
  send({ id, method, params });
  for (;;) {
    const frame = await nextFrame();
    if (!frame) throw new ModuleError("other", "Glossa closed the connection");
    if (frame.id !== id) continue;
    if (frame.error) throw new ModuleError(frame.error.kind ?? "other", frame.error.message ?? "");
    return frame.result;
  }
}

const handlers = {
  async translate(req) {
    // The count lives in Glossa's cache, so it survives this process being
    // stopped when idle.
    const count = ((await host("cache_get", { key: "count" })) ?? 0) + 1;
    await host("cache_set", { key: "count", value: count });
    await host("log", { level: "debug", message: `translate #${count}: ${req.text.length} chars` });
    return { text: req.text.split(/\s+/).filter(Boolean).reverse().join(" ") };
  },
  async detect() {
    return { lang: "en", confidence: 0.5 };
  },
};

for (;;) {
  const frame = await nextFrame();
  if (!frame) break;
  if (typeof frame.method !== "string" || frame.id === undefined) continue;
  try {
    const handler = handlers[frame.method];
    if (!handler) throw new ModuleError("unsupported", `${frame.method} is not supported by this module`);
    send({ id: frame.id, result: await handler(frame.params ?? {}) });
  } catch (e) {
    const error = e instanceof ModuleError ? { kind: e.kind, message: e.message } : { kind: "other", message: String(e?.stack ?? e) };
    send({ id: frame.id, error });
  }
}
