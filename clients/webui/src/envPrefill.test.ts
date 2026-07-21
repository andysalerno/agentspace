import { describe, expect, it } from "vitest";
import { getEnvValue, setEnvValue, withRequiredEnvKeys } from "./envPrefill";

describe("environment prefill helpers", () => {
  it("matches backend last-assignment-wins parsing", () => {
    expect(
      getEnvValue(
        "KERNEL_ACP_SERVER=opencode\nKERNEL_ACP_SERVER=copilot",
        "KERNEL_ACP_SERVER",
      ),
    ).toBe("copilot");
  });

  it("updates every duplicate assignment consistently", () => {
    expect(
      setEnvValue(
        "KERNEL_ACP_SERVER=opencode\nKERNEL_ACP_SERVER=custom",
        "KERNEL_ACP_SERVER",
        "copilot",
      ),
    ).toBe("KERNEL_ACP_SERVER=copilot\nKERNEL_ACP_SERVER=copilot");
  });

  it("prefills the ACP server and model keys", () => {
    expect(withRequiredEnvKeys("", "acp")).toBe(
      "KERNEL_ACP_SERVER=\nKERNEL_ACP_MODEL_NAME=\n",
    );
  });
});
