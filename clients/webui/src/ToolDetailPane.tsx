import Editor from "@monaco-editor/react";
import type { ToolCall } from "./types";
import { Button } from "./fluent";

type ToolDetailPaneProps = {
    toolCall: ToolCall;
    onClose: () => void;
};

function editorTheme(): string {
    return document.documentElement.getAttribute("data-theme") === "dark"
        ? "vs-dark"
        : "light";
}

export default function ToolDetailPane({ toolCall, onClose }: ToolDetailPaneProps) {
    return (
        <div className="tool-detail-overlay" onClick={onClose}>
            <div className="tool-detail-pane" onClick={(e) => e.stopPropagation()}>
                <div className="tool-detail-header">
                    <h3>⚙ {toolCall.tool}</h3>
                    <Button className="icon-button" type="button" onClick={onClose}>
                        ×
                    </Button>
                </div>
                <div className="tool-detail-editors">
                    <div className="tool-detail-section">
                        <label>Input</label>
                        <Editor
                            height="200px"
                            language="json"
                            value={toolCall.input ?? ""}
                            theme={editorTheme()}
                            options={{
                                readOnly: true,
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
                    <div className="tool-detail-section">
                        <label>Output</label>
                        <Editor
                            height="200px"
                            language="text"
                            value={toolCall.output ?? ""}
                            theme={editorTheme()}
                            options={{
                                readOnly: true,
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
                </div>
            </div>
        </div>
    );
}
