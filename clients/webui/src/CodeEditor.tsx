import Editor from "@monaco-editor/react";

type CodeEditorProps = {
    value: string;
    /*
     * Monaco renders its own textarea, which does not consume Fluent's field
     * context, so a surrounding `Field` label never reaches it. Every call site
     * passes the label it is presented under.
     */
    ariaLabel: string;
    onChange?: (value: string) => void;
    language?: string;
    height?: string;
    readOnly?: boolean;
};

export default function CodeEditor(
    { value, ariaLabel, onChange, language = "markdown", height = "200px", readOnly = false }:
        CodeEditorProps,
) {
    return (
        <div className="editor-frame">
            <Editor
                height={height}
                language={language}
                onChange={(v) => onChange?.(v ?? "")}
                options={{
                    ariaLabel,
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
