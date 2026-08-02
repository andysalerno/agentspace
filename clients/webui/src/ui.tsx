/*
 * Patterns that recur across every management view.
 *
 * These are deliberately thin: they compose Fluent primitives and the shared
 * classes in styles.css rather than introducing a second styling system.
 */
import type { FormEvent, ReactElement, ReactNode } from "react";
import { useEffect, useRef } from "react";
import { ArrowClockwise20Regular, MoreHorizontal20Regular } from "@fluentui/react-icons";
import {
    Button,
    Dialog,
    DialogActions,
    DialogBody,
    DialogContent,
    DialogSurface,
    DialogTitle,
    Menu,
    MenuItem,
    MenuList,
    MenuPopover,
    MenuTrigger,
    Spinner,
    Tooltip,
} from "./fluent";

/** Sticky page title bar. `actions` holds at most one primary button. */
export function ViewHeader(
    { title, description, actions }: {
        title: string;
        description?: ReactNode;
        actions?: ReactNode;
    },
) {
    return (
        <header className="view-header">
            <div className="view-header-heading">
                <h2>{title}</h2>
                {description !== undefined && (
                    <div className="view-header-description">{description}</div>
                )}
            </div>
            {actions !== undefined && <div className="view-header-actions">{actions}</div>}
        </header>
    );
}

export type RowAction = {
    key: string;
    label: string;
    icon?: ReactElement;
    onClick: () => void;
    disabled?: boolean;
    destructive?: boolean;
    /** Renders above the following item with a separator. */
    separatorBefore?: boolean;
};

/**
 * One inline button plus an overflow menu. Row action sets grow over time, so
 * everything past the primary action lives in the menu and can never push the
 * table out of its container.
 */
export function RowActions(
    { primary, items }: { primary?: RowAction; items: RowAction[] },
) {
    return (
        <div className="row-actions">
            {primary !== undefined && (
                <Button
                    disabled={primary.disabled}
                    icon={primary.icon}
                    onClick={primary.onClick}
                    size="small"
                >
                    {primary.label}
                </Button>
            )}
            {items.length > 0 && (
                <Menu positioning="below-end">
                    <MenuTrigger disableButtonEnhancement>
                        <Tooltip content="More actions" relationship="label">
                            <Button
                                appearance="subtle"
                                icon={<MoreHorizontal20Regular />}
                                size="small"
                            />
                        </Tooltip>
                    </MenuTrigger>
                    <MenuPopover>
                        <MenuList>
                            {items.map((item) => (
                                <MenuItem
                                    disabled={item.disabled}
                                    icon={item.icon}
                                    key={item.key}
                                    onClick={item.onClick}
                                    style={item.destructive
                                        ? { color: "var(--danger)" }
                                        : undefined}
                                >
                                    {item.label}
                                </MenuItem>
                            ))}
                        </MenuList>
                    </MenuPopover>
                </Menu>
            )}
        </div>
    );
}

export type StatusTone = "ok" | "warn" | "error" | "accent" | "neutral";

/** Dot plus label. Quieter and more scannable than a wall of coloured pills. */
export function StatusBadge({ tone, label }: { tone: StatusTone; label: string }) {
    return (
        <span className={`status-badge ${tone === "neutral" ? "" : tone}`}>{label}</span>
    );
}

export function EmptyState(
    { icon, title, description, action }: {
        icon?: ReactNode;
        title: string;
        description?: string;
        action?: ReactNode;
    },
) {
    return (
        <div className="empty-state">
            {icon !== undefined && <div className="empty-state-icon">{icon}</div>}
            <div className="empty-state-title">{title}</div>
            {description !== undefined && (
                <p className="empty-state-description">{description}</p>
            )}
            {action !== undefined && <div className="empty-state-actions">{action}</div>}
        </div>
    );
}

/** Modal create/edit form. Keeps long forms out of the page flow. */
export function FormDialog(
    { open, onOpenChange, title, submitLabel, busy, wide, onSubmit, children }: {
        open: boolean;
        onOpenChange: (open: boolean) => void;
        title: string;
        submitLabel: string;
        busy?: boolean;
        wide?: boolean;
        onSubmit: () => void;
        children: ReactNode;
    },
) {
    function handleSubmit(event: FormEvent) {
        event.preventDefault();
        onSubmit();
    }

    return (
        <Dialog
            modalType="modal"
            onOpenChange={(_, data) => onOpenChange(data.open)}
            open={open}
        >
            <DialogSurface className={wide === true ? "form-dialog-wide" : "form-dialog"}>
                <form onSubmit={handleSubmit}>
                    <DialogBody>
                        <DialogTitle>{title}</DialogTitle>
                        <DialogContent className="dialog-scroll">{children}</DialogContent>
                        <DialogActions>
                            <Button onClick={() => onOpenChange(false)} type="button">
                                Cancel
                            </Button>
                            <Button appearance="primary" disabled={busy} type="submit">
                                {submitLabel}
                            </Button>
                        </DialogActions>
                    </DialogBody>
                </form>
            </DialogSurface>
        </Dialog>
    );
}

/** Centred spinner for the first load of a view. */
export function LoadingState({ label }: { label: string }) {
    return (
        <div className="empty-state full-height">
            <Spinner label={label} size="small" />
        </div>
    );
}

/** Read-only log console. Shared by gateways and running kernels. */
export function LogsDialog(
    { open, title, lines, loading, onClose, onRefresh, toolbar }: {
        open: boolean;
        title: string;
        lines: string[];
        loading?: boolean;
        onClose: () => void;
        onRefresh?: () => void;
        toolbar?: ReactNode;
    },
) {
    const bodyRef = useRef<HTMLPreElement>(null);

    useEffect(() => {
        const node = bodyRef.current;
        if (node !== null) {
            node.scrollTop = node.scrollHeight;
        }
    }, [lines]);

    return (
        <Dialog
            modalType="modal"
            onOpenChange={(_, data) => {
                if (!data.open) onClose();
            }}
            open={open}
        >
            <DialogSurface className="form-dialog-wide">
                <DialogBody>
                    <DialogTitle>{title}</DialogTitle>
                    <DialogContent>
                        {(toolbar !== undefined || onRefresh !== undefined) && (
                            <div className="log-toolbar">
                                {toolbar}
                                {onRefresh !== undefined && (
                                    <Button
                                        disabled={loading === true}
                                        icon={<ArrowClockwise20Regular />}
                                        onClick={onRefresh}
                                        size="small"
                                        style={{ marginLeft: "auto" }}
                                    >
                                        Refresh
                                    </Button>
                                )}
                            </div>
                        )}
                        <pre className="log-viewer" ref={bodyRef}>
                            {lines.length > 0
                                ? lines.join("\n")
                                : (loading === true ? "Loading…" : "No log output.")}
                        </pre>
                    </DialogContent>
                    <DialogActions>
                        <Button onClick={onClose}>Close</Button>
                    </DialogActions>
                </DialogBody>
            </DialogSurface>
        </Dialog>
    );
}
