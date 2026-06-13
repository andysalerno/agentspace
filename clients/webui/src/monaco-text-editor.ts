const MONACO_BASE_PATH = "/monaco/vs";
const EDITOR_SOURCE = "monaco-text-editor";
const HOST_SOURCE = "monaco-text-editor-host";

type EditorMessage =
  | {
      source: typeof EDITOR_SOURCE;
      id: string;
      type: "ready";
    }
  | {
      source: typeof EDITOR_SOURCE;
      id: string;
      type: "change";
      value: string;
    }
  | {
      source: typeof EDITOR_SOURCE;
      id: string;
      type: "error";
      message: string;
    };

let nextEditorId = 0;

class MonacoTextEditor extends HTMLElement {
  static get observedAttributes(): string[] {
    return ["value", "language", "theme"];
  }

  private readonly editorId = `monaco-editor-${nextEditorId++}`;
  private readonly iframe = document.createElement("iframe");
  private valueInternal = "";
  private languageInternal = "plaintext";
  private themeInternal = "light";
  private ready = false;

  constructor() {
    super();
    this.iframe.part.add("frame");
    this.iframe.title = this.getAttribute("label") ?? "Code editor";
    this.iframe.srcdoc = createFrameHtml(this.editorId);
  }

  get value(): string {
    return this.valueInternal;
  }

  set value(value: string) {
    const nextValue = value ?? "";
    if (this.valueInternal === nextValue) {
      return;
    }
    this.valueInternal = nextValue;
    this.postConfig();
  }

  get language(): string {
    return this.languageInternal;
  }

  set language(value: string) {
    const nextLanguage = value || "plaintext";
    if (this.languageInternal === nextLanguage) {
      return;
    }
    this.languageInternal = nextLanguage;
    this.postConfig();
  }

  get theme(): string {
    return this.themeInternal;
  }

  set theme(value: string) {
    const nextTheme = value === "dark" || value === "vs-dark" ? "dark" : "light";
    if (this.themeInternal === nextTheme) {
      return;
    }
    this.themeInternal = nextTheme;
    this.postConfig();
  }

  connectedCallback(): void {
    window.addEventListener("message", this.handleMessage);
    if (!this.iframe.isConnected) {
      this.append(this.iframe);
    }
    this.postConfig();
  }

  disconnectedCallback(): void {
    window.removeEventListener("message", this.handleMessage);
  }

  attributeChangedCallback(name: string, _oldValue: string | null, newValue: string | null): void {
    if (name === "value") {
      this.value = newValue ?? "";
    } else if (name === "language") {
      this.language = newValue ?? "plaintext";
    } else if (name === "theme") {
      this.theme = newValue ?? "light";
    }
  }

  private readonly handleMessage = (event: MessageEvent<unknown>): void => {
    if (event.source !== this.iframe.contentWindow || !isEditorMessage(event.data)) {
      return;
    }

    const message = event.data;
    if (message.id !== this.editorId) {
      return;
    }

    if (message.type === "ready") {
      this.ready = true;
      this.postConfig();
      return;
    }

    if (message.type === "change") {
      this.valueInternal = message.value;
      this.dispatchEvent(new Event("input", { bubbles: true, composed: true }));
      this.dispatchEvent(new Event("change", { bubbles: true, composed: true }));
      return;
    }

    this.dispatchEvent(
      new CustomEvent("editor-error", {
        bubbles: true,
        composed: true,
        detail: message.message,
      }),
    );
  };

  private postConfig(): void {
    const target = this.iframe.contentWindow;
    if (!target || !this.isConnected) {
      return;
    }

    target.postMessage(
      {
        source: HOST_SOURCE,
        id: this.editorId,
        type: "config",
        value: this.valueInternal,
        language: this.languageInternal,
        theme: this.themeInternal === "dark" ? "vs-dark" : "vs",
        ready: this.ready,
      },
      "*",
    );
  }
}

customElements.define("monaco-text-editor", MonacoTextEditor);

function createFrameHtml(editorId: string): string {
  const frameScript = `
const EDITOR_ID = ${JSON.stringify(editorId)};
const HOST_SOURCE = ${JSON.stringify(HOST_SOURCE)};
const EDITOR_SOURCE = ${JSON.stringify(EDITOR_SOURCE)};
let editor = null;
let monacoInstance = null;
let pendingConfig = {
  value: "",
  language: "plaintext",
  theme: "vs"
};

function post(type, payload) {
  parent.postMessage({
    source: EDITOR_SOURCE,
    id: EDITOR_ID,
    type,
    ...payload
  }, "*");
}

function applyConfig(config) {
  pendingConfig = {
    value: typeof config.value === "string" ? config.value : pendingConfig.value,
    language: typeof config.language === "string" ? config.language : pendingConfig.language,
    theme: typeof config.theme === "string" ? config.theme : pendingConfig.theme
  };

  if (!editor || !monacoInstance) {
    return;
  }

  if (editor.getValue() !== pendingConfig.value) {
    editor.setValue(pendingConfig.value);
  }
  const model = editor.getModel();
  if (model) {
    monacoInstance.editor.setModelLanguage(model, pendingConfig.language);
  }
  monacoInstance.editor.setTheme(pendingConfig.theme);
  editor.layout();
}

window.addEventListener("message", (event) => {
  const data = event.data;
  if (!data || data.source !== HOST_SOURCE || data.id !== EDITOR_ID || data.type !== "config") {
    return;
  }
  applyConfig(data);
});

const loader = document.createElement("script");
loader.src = "${MONACO_BASE_PATH}/loader.js";
loader.onload = () => {
  require.config({ paths: { vs: "${MONACO_BASE_PATH}" } });
  require(["vs/editor/editor.main"], (monaco) => {
    monacoInstance = monaco;
    editor = monaco.editor.create(document.getElementById("editor"), {
      value: pendingConfig.value,
      language: pendingConfig.language,
      theme: pendingConfig.theme,
      automaticLayout: true,
      minimap: { enabled: false },
      scrollBeyondLastLine: false,
      wordWrap: "on"
    });
    editor.onDidChangeModelContent(() => post("change", { value: editor.getValue() }));
    applyConfig(pendingConfig);
    post("ready", {});
  }, (error) => {
    post("error", { message: error instanceof Error ? error.message : String(error) });
  });
};
loader.onerror = () => post("error", { message: "Failed to load Monaco editor assets." });
document.head.append(loader);
`;

  return `<!doctype html>
<html>
  <head>
    <meta charset="utf-8">
    <style>
      html,
      body,
      #editor {
        height: 100%;
        margin: 0;
        overflow: hidden;
      }

      body {
        background: transparent;
      }
    </style>
  </head>
  <body>
    <div id="editor"></div>
    <script>${frameScript.replaceAll("</script", "<\\/script")}</script>
  </body>
</html>`;
}

function isEditorMessage(value: unknown): value is EditorMessage {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const record = value as Record<string, unknown>;
  if (record.source !== EDITOR_SOURCE || typeof record.id !== "string") {
    return false;
  }
  if (record.type === "ready") {
    return true;
  }
  if (record.type === "change") {
    return typeof record.value === "string";
  }
  if (record.type === "error") {
    return typeof record.message === "string";
  }
  return false;
}
