// The typhoon playground: an editor on the left, the compiler's output on the
// right, and the real typhoon frontend compiled to WebAssembly in between.
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
let debounce = null;

function scheduleCompile() {
  clearTimeout(debounce);
  debounce = setTimeout(compileNow, DEBOUNCE_MS);
}

// Ctrl/Cmd+Enter compiles immediately. Capturing at the document means the
// binding also works in the textarea fallback, and that CodeMirror's own
// Mod-Enter never sees it.
document.addEventListener(
  "keydown",
  (event) => {
    if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
      event.preventDefault();
      event.stopPropagation();
      clearTimeout(debounce);
      compileNow();
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
  editor = await createEditor(els.editor, FALLBACK_SOURCE, scheduleCompile);

  setStatus("busy", "loading the compiler…");
  try {
    const wasm = await import("./pkg/typhoon_playground.js");
    await wasm.default();
    compileFn = wasm.compile;
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
