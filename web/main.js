// The typhoon playground: an editor on the left, the compiler's output on the
// right, and the real typhoon compiler compiled to WebAssembly in between.
//
// Programs run here too, but not in this file: `Run` compiles with the wasm
// backend (DESIGN 6.2.1) and hands the module to ./worker.js, because a
// Typhoon `while True:` would freeze a tab that ran it on the main thread.
//
// No bundler and no npm: this file is an ES module the browser loads directly,
// CodeMirror comes from a CDN (with a <textarea> fallback when it does not),
// and the wasm glue in ./pkg is produced by web/build.sh.

// CodeMirror is split over several packages that must all end up as *one*
// instance: extensions are matched by object identity, so a second copy of
// @codemirror/language means the highlighter silently stops highlighting (and
// a second @codemirror/view makes `basicSetup` throw "Unrecognized extension
// value"). `codemirror` depends on `@codemirror/language@^6.0.0`, so asking
// esm.sh for that exact range gets the module it already loaded — do not pin
// this to an exact version, and do not use jsDelivr's `+esm`, which resolves a
// different @codemirror/view for every package.
const CDN = {
  codemirror: "https://esm.sh/codemirror@6.0.2",
  language: "https://esm.sh/@codemirror/language@^6.0.0",
  python: "https://esm.sh/@codemirror/legacy-modes@6.5.1/mode/python",
};

const DEFAULT_EXAMPLE = "fib.ty";
const DEBOUNCE_MS = 150;

// How long a program may run before the worker is terminated. Long enough for
// the slowest example (fib(30) is a few hundred milliseconds), short enough
// that an accidental infinite loop is an inconvenience rather than a hang.
const RUN_TIMEOUT_MS = 10_000;

// A runaway `print` loop can emit megabytes a second; past this much the pane
// stops growing and says so, so that the page stays responsive enough to show
// the termination note.
const MAX_OUTPUT_CHARS = 200_000;

// Resolved against this module rather than against the page, so that the
// playground keeps working when it is not served from the site root.
const RUNTIME_URL = new URL("./pkg/typhoon_rt.wasm", import.meta.url).href;
const WORKER_URL = new URL("./worker.js", import.meta.url);

const OUTPUT_PLACEHOLDER =
  "Nothing has run yet. Press Run (Ctrl/\u2318+Shift+Enter) to compile with the " +
  "wasm backend and execute the program here.";

// Shown when web/examples/ has not been generated (build.sh copies it there).
const FALLBACK_SOURCE = `fn fib(n: int) -> int:
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)

fn main():
    print(fib(30))
    for i in range(5):
        print(fib(i))
`;

const els = {
  editor: document.getElementById("editor"),
  examples: document.getElementById("example-select"),
  run: document.getElementById("run"),
  description: document.getElementById("example-description"),
  diagCount: document.getElementById("diag-count"),
  state: document.getElementById("status-state"),
  timings: document.getElementById("status-timings"),
  status: document.getElementById("status"),
  tabs: Array.from(document.querySelectorAll(".tab")),
  panel: (name) => document.getElementById(`panel-${name}`),
};

// --------------------------------------------------------------------- tabs

function selectTab(name) {
  for (const tab of els.tabs) {
    const active = tab.dataset.tab === name;
    tab.setAttribute("aria-selected", String(active));
    els.panel(tab.dataset.tab).hidden = !active;
  }
}

for (const tab of els.tabs) {
  tab.addEventListener("click", () => selectTab(tab.dataset.tab));
}
els.status.addEventListener("click", () => selectTab("diagnostics"));

function setPanel(name, text, placeholder) {
  const panel = els.panel(name);
  const empty = !text;
  panel.textContent = empty ? placeholder : text;
  panel.classList.toggle("empty", empty);
}

// ------------------------------------------------------------------ editor

// Both editor implementations expose the same three methods, so the rest of
// the page never learns which one it got.
async function createEditor(host, initialDoc, onChange) {
  try {
    const [cm, language, mode] = await Promise.all([
      import(CDN.codemirror),
      import(CDN.language),
      import(CDN.python),
    ]);

    // Typhoon is Python-flavoured syntax with `fn` instead of `def`
    // (DESIGN 3.2), so the legacy Python mode plus one extra keyword is a
    // faithful highlighter.
    const typhoon = language.StreamLanguage.define(
      mode.mkPython({ extra_keywords: ["fn"] }),
    );

    const editor = new cm.EditorView({
      doc: initialDoc,
      parent: host,
      extensions: [
        cm.basicSetup,
        typhoon,
        // Four spaces, and never a tab: the lexer rejects tabs outright
        // (DESIGN 3.1).
        language.indentUnit.of("    "),
        cm.EditorView.lineWrapping,
        cm.EditorView.updateListener.of((update) => {
          if (update.docChanged) onChange();
        }),
      ],
    });

    return {
      getValue: () => editor.state.doc.toString(),
      setValue: (text) => {
        editor.dispatch({
          changes: { from: 0, to: editor.state.doc.length, insert: text },
          selection: { anchor: 0 },
        });
      },
      focus: () => editor.focus(),
    };
  } catch (err) {
    console.warn("CodeMirror could not be loaded, falling back to a textarea", err);
    const area = document.createElement("textarea");
    area.spellcheck = false;
    area.value = initialDoc;
    area.setAttribute("aria-label", "Typhoon source");
    area.addEventListener("input", onChange);
    host.appendChild(area);
    return {
      getValue: () => area.value,
      setValue: (text) => {
        area.value = text;
      },
      focus: () => area.focus(),
    };
  }
}

// ------------------------------------------------------------------- state

let editor = null;
let compileFn = null;
let compileWasmFn = null;
let debounce = null;

function scheduleCompile() {
  clearTimeout(debounce);
  debounce = setTimeout(compileNow, DEBOUNCE_MS);
}

// Ctrl/Cmd+Enter compiles immediately, and with Shift it runs. Capturing at
// the document means the bindings also work in the textarea fallback, and that
// CodeMirror's own Mod-Enter never sees them.
document.addEventListener(
  "keydown",
  (event) => {
    if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
      event.preventDefault();
      event.stopPropagation();
      clearTimeout(debounce);
      if (event.shiftKey) {
        runNow();
      } else {
        compileNow();
      }
    }
  },
  true,
);

function setStatus(cls, text) {
  els.state.className = `state ${cls}`;
  els.state.textContent = text;
}

function compileNow() {
  if (!compileFn || !editor) return;

  let result;
  try {
    result = JSON.parse(compileFn(editor.getValue()));
  } catch (err) {
    // A panic in the compiler traps the wasm module; say so instead of
    // silently showing stale output.
    console.error(err);
    setStatus("error", "the compiler crashed — see the browser console");
    setPanel("diagnostics", String(err), "");
    selectTab("diagnostics");
    return;
  }

  const errors = result.diagnostics.filter((d) => d.startsWith("error")).length;
  const warnings = result.diagnostics.length - errors;

  setPanel(
    "diagnostics",
    result.diagnostics.join("\n\n"),
    "No diagnostics. The program compiles.",
  );
  setPanel(
    "llvm_ir",
    result.llvm_ir,
    "No LLVM IR: the program did not compile. See the Diagnostics tab.",
  );
  setPanel("ast", result.ast, "");
  setPanel("tokens", result.tokens, "");
  setPanel(
    "hir",
    result.hir,
    "No typed HIR: the program did not get past parsing or type checking.",
  );

  els.diagCount.textContent = errors > 0 ? String(errors) : "";

  const plural = (n, word) => `${n} ${word}${n === 1 ? "" : "s"}`;
  if (result.ok) {
    const lines = result.llvm_ir ? result.llvm_ir.split("\n").length : 0;
    setStatus(
      "ok",
      warnings > 0
        ? `ok — ${plural(warnings, "warning")}, ${plural(lines, "line")} of LLVM IR`
        : `ok — ${plural(lines, "line")} of LLVM IR`,
    );
  } else {
    setStatus(
      "error",
      warnings > 0
        ? `${plural(errors, "error")}, ${plural(warnings, "warning")}`
        : plural(errors, "error"),
    );
  }

  const t = result.timings_ms;
  const total = t.lex + t.parse + t.sema + t.codegen;
  const ms = (value) => value.toFixed(2);
  els.timings.textContent =
    `lex ${ms(t.lex)} · parse ${ms(t.parse)} · sema ${ms(t.sema)} · ` +
    `codegen ${ms(t.codegen)} · total ${ms(total)} ms`;
}

// --------------------------------------------------------------------- run
//
// Running a program is a second compilation — the wasm backend, not the LLVM
// one — plus a worker that instantiates the module against the Typhoon runtime
// in web/pkg/typhoon_rt.wasm. The worker exists to be killable: `terminate()`
// is the only way to stop wasm that has decided not to come back.

let worker = null;
let runTimer = null;

// Output arrives one flush at a time and is appended on an animation frame:
// merging what a print loop emitted between two frames turns thousands of DOM
// mutations into one.
let queued = [];
let flushHandle = null;
let outputChars = 0;
let outputTruncated = false;
let outputAtLineStart = true;

function clearOutput() {
  const panel = els.panel("output");
  panel.textContent = "";
  panel.classList.remove("empty");
  if (flushHandle !== null) {
    cancelAnimationFrame(flushHandle);
    flushHandle = null;
  }
  queued = [];
  outputChars = 0;
  outputTruncated = false;
  outputAtLineStart = true;
}

function queueOutput(kind, text) {
  if (outputTruncated) return;
  queued.push([kind, text]);
  if (flushHandle === null) {
    flushHandle = requestAnimationFrame(flushOutput);
  }
}

function flushOutput() {
  if (flushHandle !== null) {
    cancelAnimationFrame(flushHandle);
    flushHandle = null;
  }
  if (queued.length === 0) return;

  const panel = els.panel("output");
  // Follow the output only while the reader is at the end, so that scrolling
  // back into a long run is not undone by the next line.
  const following = panel.scrollHeight - panel.scrollTop - panel.clientHeight < 24;
  for (const [kind, text] of queued) {
    appendOutput(kind, text);
  }
  queued = [];
  if (following) {
    panel.scrollTop = panel.scrollHeight;
  }
}

function appendOutput(kind, text) {
  if (outputTruncated) return;

  let chunk = text;
  if (outputChars + chunk.length > MAX_OUTPUT_CHARS) {
    chunk = chunk.slice(0, MAX_OUTPUT_CHARS - outputChars);
    outputTruncated = true;
  }
  if (chunk.length > 0) {
    outputChars += chunk.length;
    outputAtLineStart = chunk.endsWith("\n");
    const panel = els.panel("output");
    if (kind === "stderr") {
      const span = document.createElement("span");
      span.className = "stream-err";
      span.textContent = chunk;
      panel.appendChild(span);
    } else {
      // A text node, never innerHTML: this is the program's output, and the
      // program is whatever the visitor typed.
      panel.appendChild(document.createTextNode(chunk));
    }
  }
  if (outputTruncated) {
    outputNote(`--- output truncated after ${MAX_OUTPUT_CHARS} characters ---`, "bad");
  }
}

/** Appends one line of the playground's own commentary, on its own line. */
function outputNote(text, cls) {
  const span = document.createElement("span");
  span.className = `run-note ${cls}`;
  // The note carries its own newlines, so that it never shares a line with a
  // program that did not end its output with one.
  span.textContent = `${outputAtLineStart ? "" : "\n"}${text}\n`;
  outputAtLineStart = true;

  const panel = els.panel("output");
  panel.appendChild(span);
  // Always scrolled to: the exit line is the point of the pane.
  panel.scrollTop = panel.scrollHeight;
}

/** Bytes of a standard-base64 string; `wasm` from `compile_wasm` is one. */
function decodeBase64(text) {
  const binary = atob(text);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

/** Kills the current run, if any, and forgets it. */
function stopRun() {
  if (runTimer !== null) {
    clearTimeout(runTimer);
    runTimer = null;
  }
  if (worker !== null) {
    worker.terminate();
    worker = null;
  }
}

function runNow() {
  if (!compileWasmFn || !editor) return;

  // Whatever was running is abandoned: the pane is about to be cleared, and a
  // second worker would interleave its output with the first one's.
  stopRun();
  clearTimeout(debounce);
  // Keep the other tabs showing the program that is about to run.
  compileNow();

  let result;
  try {
    result = JSON.parse(compileWasmFn(editor.getValue()));
  } catch (err) {
    console.error(err);
    setStatus("error", "the compiler crashed — see the browser console");
    setPanel("diagnostics", String(err), "");
    selectTab("diagnostics");
    return;
  }

  if (!result.ok) {
    // The two backends run the same frontend, so these are the diagnostics
    // `compileNow` just showed; re-rendering them keeps the pane right even if
    // one backend ever rejects what the other accepts.
    setPanel("diagnostics", result.diagnostics.join("\n\n"), "");
    selectTab("diagnostics");
    setStatus("error", "the program does not compile");
    return;
  }

  clearOutput();
  selectTab("output");
  setStatus("busy", "running…");

  try {
    worker = new Worker(WORKER_URL, { type: "module" });
  } catch (err) {
    // Module workers are the one modern feature the page cannot do without;
    // say so plainly rather than leaving an empty pane behind.
    console.error(err);
    worker = null;
    outputNote(`--- this browser cannot run programs: ${err} ---`, "bad");
    setStatus("error", "this browser has no module workers");
    return;
  }

  const started = performance.now();

  worker.onmessage = (event) => {
    const message = event.data;
    if (message.type === "stdout" || message.type === "stderr") {
      queueOutput(message.type, message.text);
      return;
    }
    if (message.type !== "exit") return;

    // `exit` is the last message the worker sends, and messages are delivered
    // in order, so everything the program wrote has already been queued.
    stopRun();
    flushOutput();
    if (message.error) {
      outputNote(`--- could not run: ${message.error} ---`, "bad");
      setStatus("error", "could not run the program");
      return;
    }
    const suffix = message.note ? ` (${message.note})` : "";
    const footer = `exit ${message.code} · ${Math.round(message.ms)} ms${suffix}`;
    const ok = message.code === 0;
    outputNote(footer, ok ? "ok" : "bad");
    setStatus(ok ? "ok" : "error", footer);
  };

  worker.onerror = (event) => {
    // Fires when worker.js itself cannot load — a missing web/pkg, or a
    // browser that ignored `type: "module"`.
    event.preventDefault();
    console.error(event.message ?? event);
    stopRun();
    flushOutput();
    // Chrome hides the reason a worker script failed to load, so the note
    // names the likeliest cause when the event carries no message.
    const detail = event.message ? `: ${event.message}` : " (has web/build.sh been run?)";
    outputNote(`--- could not start the runner${detail} ---`, "bad");
    setStatus("error", "could not start the runner — see the browser console");
  };

  // The module is transferred rather than copied; the page has no use for it
  // afterwards.
  const program = decodeBase64(result.wasm);
  worker.postMessage({ wasm: program, runtimeUrl: RUNTIME_URL }, [program.buffer]);

  runTimer = setTimeout(() => {
    stopRun();
    flushOutput();
    const seconds = ((performance.now() - started) / 1000).toFixed(1);
    outputNote(`--- terminated after ${seconds}s ---`, "bad");
    setStatus("error", `terminated after ${seconds}s`);
  }, RUN_TIMEOUT_MS);
}

els.run.addEventListener("click", runNow);

// ----------------------------------------------------------------- startup

async function loadExamples() {
  const response = await fetch("./examples/index.json");
  if (!response.ok) throw new Error(`examples/index.json: ${response.status}`);
  return await response.json();
}

async function setupExamples() {
  let examples;
  try {
    examples = await loadExamples();
  } catch (err) {
    console.warn("no examples/index.json; run web/build.sh to generate it", err);
    els.examples.innerHTML = "";
    const option = document.createElement("option");
    option.textContent = "(examples unavailable)";
    els.examples.appendChild(option);
    els.examples.disabled = true;
    return;
  }

  els.examples.innerHTML = "";
  for (const example of examples) {
    const option = document.createElement("option");
    option.value = example.file;
    option.textContent = `${example.name} — ${example.milestone}`;
    option.dataset.description = example.description ?? "";
    els.examples.appendChild(option);
  }

  const load = async (file) => {
    const response = await fetch(`./examples/${file}`);
    if (!response.ok) return;
    editor.setValue(await response.text());
    const selected = els.examples.selectedOptions[0];
    els.description.textContent = selected ? selected.dataset.description : "";
    compileNow();
  };

  els.examples.addEventListener("change", () => load(els.examples.value));

  const initial = examples.find((e) => e.file === DEFAULT_EXAMPLE) ?? examples[0];
  if (initial) {
    els.examples.value = initial.file;
    await load(initial.file);
  }
}

async function main() {
  selectTab("llvm_ir");
  setPanel("output", "", OUTPUT_PLACEHOLDER);
  // Armed once the compiler is in memory; a click before that would do
  // nothing, and a dead button says so better than one that ignores you.
  els.run.disabled = true;
  editor = await createEditor(els.editor, FALLBACK_SOURCE, scheduleCompile);

  setStatus("busy", "loading the compiler…");
  try {
    const wasm = await import("./pkg/typhoon_playground.js");
    await wasm.default();
    compileFn = wasm.compile;
    compileWasmFn = wasm.compile_wasm;
    els.run.disabled = false;
  } catch (err) {
    console.error(err);
    setStatus("error", "could not load the compiler (web/pkg is missing?)");
    setPanel(
      "diagnostics",
      `${err}\n\nBuild the WebAssembly module with web/build.sh, then serve the ` +
        `web/ directory (python3 -m http.server -d web 8000).`,
      "",
    );
    selectTab("diagnostics");
    return;
  }

  await setupExamples();
  compileNow();
  editor.focus();
}

main();
