import { useCallback, useMemo, useState } from "react";
import type { ReactNode } from "react";
import { ErrorContext } from "./useErrorContext";

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
