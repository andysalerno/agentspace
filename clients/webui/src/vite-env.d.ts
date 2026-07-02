/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_WEBUI_VERSION?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
