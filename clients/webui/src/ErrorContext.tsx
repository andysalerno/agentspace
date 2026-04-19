import { createContext, useCallback, useContext, useMemo, useState } from "react";
import type { ReactNode } from "react";

type ErrorContextValue = {
  error: string | null;
  setError: (message: string | null) => void;
  clearError: () => void;
  reportError: (err: unknown) => void;
};

const ErrorContext = createContext<ErrorContextValue | null>(null);

export function ErrorProvider({ children }: { children: ReactNode }) {
  const [error, setError] = useState<string | null>(null);
  const clearError = useCallback(() => setError(null), []);
  const reportError = useCallback((err: unknown) => {
    setError(err instanceof Error ? err.message : String(err));
  }, []);
  const value = useMemo(
    () => ({ error, setError, clearError, reportError }),
    [error, clearError, reportError],
  );
  return <ErrorContext.Provider value={value}>{children}</ErrorContext.Provider>;
}

export function useErrorContext(): ErrorContextValue {
  const ctx = useContext(ErrorContext);
  if (ctx === null) {
    throw new Error("useErrorContext must be used inside ErrorProvider");
  }
  return ctx;
}
