import Editor from "@monaco-editor/react";

type CodeEditorProps = {
    value: string;
    onChange?: (value: string) => void;
    language?: string;
    height?: string;
    readOnly?: boolean;
};

export default function CodeEditor(
    { value, onChange, language = "markdown", height = "200px", readOnly = false }:
        CodeEditorProps,
) {
    return (
        <div className="editor-frame">
            <Editor
                height={height}
                language={language}
                onChange={(v) => onChange?.(v ?? "")}
                options={{
                    minimap: { enabled: false },
                    lineNumbers: "on",
                    scrollBeyondLastLine: false,
                    wordWrap: "on",
                    fontSize: 13,
                    tabSize: 2,
                    automaticLayout: true,
                    fixedOverflowWidgets: true,
                    renderLineHighlight: "none",
                    padding: { top: 8, bottom: 8 },
                    readOnly,
                    domReadOnly: readOnly,
                }}
                theme={document.documentElement.getAttribute("data-theme") === "dark"
                    ? "vs-dark"
                    : "light"}
                value={value}
            />
        </div>
    );
}
