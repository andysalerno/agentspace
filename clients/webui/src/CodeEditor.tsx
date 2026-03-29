import Editor from "@monaco-editor/react";

type CodeEditorProps = {
    value: string;
    onChange: (value: string) => void;
    language?: string;
    height?: string;
    placeholder?: string;
};

export default function CodeEditor({
    value,
    onChange,
    language = "markdown",
    height = "200px",
}: CodeEditorProps) {
    return (
        <div style={{ border: "1px solid var(--border-color)", borderRadius: "var(--radius-sm)", overflow: "hidden" }}>
            <Editor
                height={height}
                language={language}
                value={value}
                onChange={(v) => onChange(v ?? "")}
                theme={document.documentElement.getAttribute("data-theme") === "dark" ? "vs-dark" : "light"}
                options={{
                    minimap: { enabled: false },
                    lineNumbers: "on",
                    scrollBeyondLastLine: false,
                    wordWrap: "on",
                    fontSize: 13,
                    tabSize: 2,
                    automaticLayout: true,
                    fixedOverflowWidgets: true,
                }}
            />
        </div>
    );
}
