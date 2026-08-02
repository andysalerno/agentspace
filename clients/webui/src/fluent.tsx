/*
 * Thin re-export of the Fluent UI v9 vocabulary the console is allowed to use.
 *
 * Views import from here rather than from @fluentui/react-components directly,
 * so the set of components in play stays small and reviewable. Buttons keep
 * Fluent's neutral default appearance; `appearance="primary"` is set
 * deliberately, and at most once per surface.
 */
export {
    Button,
    Checkbox,
    Combobox,
    Dialog,
    DialogActions,
    DialogBody,
    DialogContent,
    DialogSurface,
    DialogTitle,
    Field,
    FluentProvider,
    Input,
    Menu,
    MenuItem,
    MenuList,
    MenuPopover,
    MenuTrigger,
    MessageBar,
    MessageBarActions,
    MessageBarBody,
    Option,
    SearchBox,
    Select,
    Spinner,
    Tab,
    TabList,
    Textarea,
    Toolbar,
    ToolbarButton,
    ToolbarDivider,
    Tooltip,
} from "@fluentui/react-components";
