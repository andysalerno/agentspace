import { createContext, useContext } from "react";

export type ErrorContextValue = {
  error: string | null;
  setError: (message: string | null) => void;
  clearError: () => void;
  reportError: (err: unknown) => void;
};

export const ErrorContext = createContext<ErrorContextValue | null>(null);

export function useErrorContext(): ErrorContextValue {
  const ctx = useContext(ErrorContext);
  if (ctx === null) {
    throw new Error("useErrorContext must be used inside ErrorProvider");
  }
  return ctx;
}
