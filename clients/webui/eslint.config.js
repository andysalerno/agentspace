import js from "@eslint/js";
import tseslint from "typescript-eslint";
import react from "eslint-plugin-react";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import globals from "globals";

export default tseslint.config(
    {
        ignores: ["dist/**", "node_modules/**", "vite.config.d.ts", "vite.config.js"],
    },
    js.configs.recommended,
    ...tseslint.configs.recommendedTypeChecked,
    {
        files: ["**/*.{ts,tsx}"],
        languageOptions: {
            parserOptions: {
                projectService: true,
                tsconfigRootDir: import.meta.dirname,
            },
            globals: {
                ...globals.browser,
            },
        },
        settings: {
            react: { version: "detect" },
        },
        plugins: {
            react,
            "react-hooks": reactHooks,
            "react-refresh": reactRefresh,
        },
        rules: {
            ...react.configs.recommended.rules,
            ...react.configs["jsx-runtime"].rules,
            ...reactHooks.configs.recommended.rules,
            "react-refresh/only-export-components": [
                "warn",
                { allowConstantExport: true },
            ],
            // Async handlers passed to JSX attributes (onClick={async () => ...})
            // are an idiomatic React pattern; only flag promise misuse outside JSX.
            "@typescript-eslint/no-misused-promises": [
                "error",
                { checksVoidReturn: { attributes: false } },
            ],
            // The React 19 "set state inside an effect" rule flags many valid
            // patterns (e.g. setting derived defaults). Keep visible but
            // non-blocking.
            "react-hooks/set-state-in-effect": "warn",
            "@typescript-eslint/consistent-type-imports": [
                "error",
                { prefer: "type-imports", fixStyle: "separate-type-imports" },
            ],
            "@typescript-eslint/no-unused-vars": [
                "error",
                { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
            ],
        },
    },
    {
        // Config files (this file, etc.) — no project-aware type checking.
        files: ["*.config.{js,ts,mjs,cjs}", "eslint.config.{js,mjs}"],
        ...tseslint.configs.disableTypeChecked,
    },
);
