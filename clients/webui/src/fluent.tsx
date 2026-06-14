import {
    Button as FluentButton,
    Checkbox as FluentCheckbox,
    FluentProvider,
    Input as FluentInput,
    Select as FluentSelect,
    Table,
    TableBody,
    TableCell,
    TableHeader,
    TableHeaderCell,
    TableRow,
    Textarea as FluentTextarea,
} from "@fluentui/react-components";
import { forwardRef } from "react";
import type {
    ButtonProps,
    CheckboxProps,
    InputProps,
    SelectProps,
    TextareaProps,
} from "@fluentui/react-components";

function classText(className: unknown): string {
    return typeof className === "string" ? className : "";
}

function buttonAppearance(className: string): ButtonProps["appearance"] {
    if (
        className.includes("secondary-button")
        || className.includes("danger-button")
        || className.includes("dismiss-button")
        || className.includes("icon-button")
        || className.includes("kebab-button")
        || className.includes("rail-delete-all-button")
        || className.includes("session-delete-button")
        || className.includes("session-item")
        || className.includes("chat-session-card")
        || className.includes("tool-call-tag")
        || className.includes("inline-tool-call")
        || className.includes("list-item")
        || className.includes("sidebar")
    ) {
        return "subtle";
    }
    return "primary";
}

function buttonSize(className: string): ButtonProps["size"] {
    return className.includes("small") || className.includes("icon-button")
        ? "small"
        : "medium";
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
    { appearance, className, size, ...props },
    ref,
) {
    const classNameValue = classText(className);
    return (
        <FluentButton
            appearance={appearance ?? buttonAppearance(classNameValue)}
            className={className}
            ref={ref}
            size={size ?? buttonSize(classNameValue)}
            {...props}
        />
    );
});

export function Input({ appearance, ...props }: InputProps) {
    return <FluentInput appearance={appearance ?? "outline"} {...props} />;
}

export function Select({ appearance, ...props }: SelectProps) {
    return <FluentSelect appearance={appearance ?? "outline"} {...props} />;
}

export function Textarea({ appearance, ...props }: TextareaProps) {
    return <FluentTextarea appearance={appearance ?? "outline"} {...props} />;
}

export function Checkbox(props: CheckboxProps) {
    return <FluentCheckbox {...props} />;
}

export {
    FluentProvider,
    Table,
    TableBody,
    TableCell,
    TableHeader,
    TableHeaderCell,
    TableRow,
};
