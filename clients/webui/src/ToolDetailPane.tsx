import type { ToolCall } from "./types";
import CodeEditor from "./CodeEditor";
import {
    Button,
    Dialog,
    DialogActions,
    DialogBody,
    DialogContent,
    DialogSurface,
    DialogTitle,
    Field,
} from "./fluent";

type ToolDetailPaneProps = {
    /** `null` keeps the dialog mounted but closed, so it can reopen instantly. */
    toolCall: ToolCall | null;
    onClose: () => void;
};

/** Read-only input and output for a single tool call. */
export default function ToolDetailPane({ toolCall, onClose }: ToolDetailPaneProps) {
    return (
        <Dialog
            modalType="modal"
            onOpenChange={(_, data) => {
                if (!data.open) onClose();
            }}
            open={toolCall !== null}
        >
            <DialogSurface className="form-dialog-wide">
                <DialogBody>
                    <DialogTitle>{toolCall?.tool ?? "Tool call"}</DialogTitle>
                    <DialogContent className="dialog-scroll">
                        <Field label="Input">
                            <CodeEditor
                                ariaLabel="Tool call input"
                                height="220px"
                                language="json"
                                readOnly
                                value={toolCall?.input ?? ""}
                            />
                        </Field>
                        <Field label="Output">
                            <CodeEditor
                                ariaLabel="Tool call output"
                                height="260px"
                                language="plaintext"
                                readOnly
                                value={toolCall?.output ?? ""}
                            />
                        </Field>
                    </DialogContent>
                    <DialogActions>
                        <Button onClick={onClose}>Close</Button>
                    </DialogActions>
                </DialogBody>
            </DialogSurface>
        </Dialog>
    );
}
