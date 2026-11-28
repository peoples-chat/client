// Issue #129: Copy on Profile hands over the plain npub that White Noise takes.
import { describe, expect, it } from "vitest";
import { copyPublicKey } from "./Profile";

const NPUB = "npub180cvv07tjdrrgpa0j7j7tmnyl2yr6yr7l8j4s3evf6u64th6gkwsyjh6w6";

describe("issue #129: copy the npub", () => {
  it("writes exactly the npub to the clipboard", async () => {
    const written: string[] = [];
    const result = await copyPublicKey(NPUB, { writeText: async (text) => void written.push(text) });
    expect(result).toBe("copied");
    expect(written).toEqual([NPUB]);
  });

  it("reports a refused clipboard", async () => {
    const result = await copyPublicKey(NPUB, {
      writeText: () => Promise.reject(new DOMException("not allowed", "NotAllowedError")),
    });
    expect(result).toBe("failed");
  });
});
