import { describe, expect, it } from "vitest";
import { sessionVscodeUrl } from "./browserUrls";

describe("sessionVscodeUrl", () => {
  it("uses the same-origin session proxy and escapes the session ID", () => {
    expect(sessionVscodeUrl("session/id")).toBe(
      "/api/sessions/session%2Fid/vscode/",
    );
  });
});
