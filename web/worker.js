// Runs one compiled Typhoon program, off the main thread.
//
// A Typhoon program is an ordinary wasm module with an ordinary `loop` in it:
// `while True: pass` would freeze the tab if it ran on the UI thread, and no
// amount of care on our side can interrupt it. So every run happens here, in a
// worker the page can `terminate()` — that is the only reliable way to stop
// running wasm.
//
// The protocol is one message in, a stream of messages out:
//
//   in   { wasm: Uint8Array, runtimeUrl: string }
//   out  { type: "stdout" | "stderr", text: string }   zero or more, in order
//   out  { type: "exit", code, ms, note?, error? }     exactly one, last
//
// `note` names an abnormal exit ("panic" or "trap"); `error` is set instead
// when the program never got to run at all.

import { PANIC_EXIT_CODE, runTyphoon } from "./pkg/loader.js";

/** Flush the buffer once it holds this many characters. */
const FLUSH_CHARS = 8192;

/** ...or once this long has passed since the last flush. */
const FLUSH_MS = 16;

// The program runs inside one synchronous call, so this worker cannot flush on
// a timer: everything is written from inside `runTyphoon`. Buffering there
// instead keeps a `for i in range(1000000): print(i)` from posting a million
// messages (which would starve the page it is trying to feed) while a program
// that prints one line a second still streams, because the time budget fires
// long before the size one does.
const pending = { type: null, text: "", last: 0 };

/** Queues `text` for the page, preserving the order of the two streams. */
function write(type, text) {
  // stdout and stderr share one buffer: flushing them independently would let
  // a panic message overtake the output printed before it.
  if (pending.type !== null && pending.type !== type) {
    flush();
  }
  pending.type = type;
  pending.text += text;
  const now = performance.now();
  if (pending.text.length >= FLUSH_CHARS || now - pending.last >= FLUSH_MS) {
    flush(now);
  }
}

/** Posts whatever is buffered, if anything. */
function flush(now) {
  if (pending.text.length === 0) {
    return;
  }
  self.postMessage({ type: pending.type, text: pending.text });
  pending.text = "";
  pending.last = now ?? performance.now();
}

/**
 * Fetches the Typhoon runtime, the module that owns the linear memory and
 * exports the `ty_*` ABI the compiled program imports.
 *
 * The bytes rather than the `Response`: `WebAssembly.compileStreaming` insists
 * on a `Content-Type: application/wasm`, which not every static file server
 * sends, and the runtime is small enough that it does not matter.
 */
async function loadRuntime(url) {
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`cannot fetch ${url}: ${response.status} ${response.statusText}`);
  }
  return new Uint8Array(await response.arrayBuffer());
}

self.onmessage = async (event) => {
  const { wasm, runtimeUrl } = event.data;
  const started = performance.now();

  let result;
  try {
    const runtime = await loadRuntime(runtimeUrl);
    result = await runTyphoon({
      runtime,
      program: wasm,
      stdout: (text) => write("stdout", text),
      stderr: (text) => write("stderr", text),
    });
  } catch (error) {
    // Instantiation failed, the runtime is missing, or the module imports
    // something this runtime does not export: none of that is the program
    // exiting, so it is reported as an error rather than as an exit code.
    flush();
    self.postMessage({
      type: "exit",
      code: null,
      ms: performance.now() - started,
      error: String(error && error.message ? error.message : error),
    });
    return;
  }

  flush();
  let note;
  if (result.trapped) {
    note = "trap";
  } else if (result.exitCode === PANIC_EXIT_CODE) {
    note = "panic";
  }
  self.postMessage({
    type: "exit",
    code: result.exitCode,
    ms: performance.now() - started,
    note,
  });
};
