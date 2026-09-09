// The Typhoon wasm loader — the single source of truth for running a compiled
// Typhoon program, shared by `typhoon build --target wasm` (which embeds this
// file in the `<name>.js` it writes) and by the browser playground (where
// `web/build.sh` copies it to `web/pkg/loader.js`).
//
// A Typhoon wasm program is two modules (DESIGN section 6.2.1):
//
//   * `typhoon_rt.wasm`, the Typhoon runtime, which owns the linear memory and
//     exports the `ty_*` C ABI. It imports `host_write` and `host_exit` from
//     `env`, because wasm32-unknown-unknown has neither stdout nor a process to
//     exit.
//   * the program itself, which imports the runtime's `memory` and the `ty_*`
//     functions it calls, and exports `_start`.
//
// So: instantiate the runtime with the two host functions, then instantiate the
// program with the runtime's exports, then call `_start`.
//
// This module is plain ES: no bundler, no dependencies, and nothing that only
// works in one of Node and the browser.

/** Exit code the runtime reports for a Typhoon panic (`ty_panic`). */
export const PANIC_EXIT_CODE = 101;

/** Exit code reported when the module traps instead of exiting. */
export const TRAP_EXIT_CODE = 134;

/**
 * Thrown by `host_exit` to unwind out of wasm. `_start` never returns
 * normally: the runtime always finishes through `ty_rt_exit`.
 */
class ExitSignal extends Error {
  constructor(code) {
    super(`typhoon: exit ${code}`);
    this.name = "ExitSignal";
    this.code = code;
  }
}

/**
 * Buffers bytes written by the program and hands complete text to a callback.
 *
 * The runtime flushes on a fixed byte budget, so a multi-byte UTF-8 sequence
 * can be split across two writes; a streaming decoder stitches them back
 * together.
 */
class OutputStream {
  constructor(sink) {
    this.sink = sink;
    this.decoder = new TextDecoder("utf-8");
  }

  write(bytes) {
    if (!this.sink) {
      return;
    }
    const text = this.decoder.decode(bytes, { stream: true });
    if (text.length > 0) {
      this.sink(text);
    }
  }

  finish() {
    if (!this.sink) {
      return;
    }
    const text = this.decoder.decode();
    if (text.length > 0) {
      this.sink(text);
    }
  }
}

/** Compiles `source`, which may be bytes, a module or a promise for either. */
async function toModule(source) {
  const resolved = await source;
  if (resolved instanceof WebAssembly.Module) {
    return resolved;
  }
  if (typeof Response !== "undefined" && resolved instanceof Response) {
    return await WebAssembly.compileStreaming(resolved);
  }
  return await WebAssembly.compile(resolved);
}

/**
 * Runs one compiled Typhoon program.
 *
 * @param {object} options
 * @param {BufferSource|WebAssembly.Module|Promise} options.runtime
 *        `typhoon_rt.wasm`.
 * @param {BufferSource|WebAssembly.Module|Promise} options.program
 *        the compiled `.ty` file.
 * @param {(text: string) => void} [options.stdout] receives standard output.
 * @param {(text: string) => void} [options.stderr] receives standard error,
 *        which is where a panic message goes.
 * @returns {Promise<{exitCode: number, trapped: boolean}>} the program's exit
 *          code: 0 normally, 101 for a panic, 134 if the module trapped.
 */
export async function runTyphoon(options) {
  const { runtime, program } = options;
  const stdout = new OutputStream(options.stdout);
  const stderr = new OutputStream(options.stderr);

  const [runtimeModule, programModule] = await Promise.all([
    toModule(runtime),
    toModule(program),
  ]);

  let memory = null;
  const env = {
    host_write(fd, ptr, len) {
      if (len === 0 || memory === null) {
        return;
      }
      const bytes = new Uint8Array(memory.buffer, ptr, len);
      // A copy, because the view dies with the next `memory.grow`.
      const copy = bytes.slice();
      if (fd === 2) {
        stderr.write(copy);
      } else {
        stdout.write(copy);
      }
    },
    host_exit(code) {
      throw new ExitSignal(code);
    },
  };

  const runtimeInstance = await WebAssembly.instantiate(runtimeModule, { env });
  const exports = runtimeInstance.exports;
  memory = exports.memory;
  if (!memory) {
    throw new Error("typhoon: the runtime module does not export its memory");
  }

  // The program imports exactly the `ty_*` functions it calls, so hand it
  // everything the runtime exports and let the engine pick.
  const programInstance = await WebAssembly.instantiate(programModule, {
    env: exports,
  });
  const start = programInstance.exports._start;
  if (typeof start !== "function") {
    throw new Error("typhoon: the program does not export _start");
  }

  let exitCode = 0;
  let trapped = false;
  try {
    start();
  } catch (error) {
    if (error instanceof ExitSignal) {
      exitCode = error.code;
    } else {
      // Anything that is not our exit signal ended the program the hard way.
      // `WebAssembly.RuntimeError` covers `unreachable` and out-of-bounds
      // accesses, but not everything: V8 reports running out of stack — which
      // a deeply recursive Typhoon program can do, since the wasm stack is much
      // smaller than a native one — as a plain `RangeError`, and other engines
      // spell it differently again. Report them all the same way rather than
      // letting one escape and take the page (or the Node process) down.
      trapped = true;
      exitCode = TRAP_EXIT_CODE;
      const message = error && error.message ? error.message : String(error);
      stderr.write(new TextEncoder().encode(`trap: ${message}\n`));
    }
  } finally {
    stdout.finish();
    stderr.finish();
  }

  return { exitCode, trapped };
}

/**
 * Reads a `.wasm` file, in Node and in the browser alike.
 *
 * `fetch` cannot read `file:` URLs under Node, so Node goes through `fs`; the
 * dynamic import keeps the browser from ever seeing `node:fs/promises`.
 */
export async function loadWasm(url) {
  if (typeof process !== "undefined" && process.versions && process.versions.node) {
    const { readFile } = await import("node:fs/promises");
    const bytes = await readFile(url);
    return new Uint8Array(bytes);
  }
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`typhoon: cannot fetch ${url}: ${response.status}`);
  }
  return await WebAssembly.compileStreaming(response);
}
