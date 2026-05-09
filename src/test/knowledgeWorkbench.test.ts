import { describe, expect, it } from "vitest";
import { resolveKnowledgeImageUrls } from "../components/knowledge/knowledgeImageUrls";

describe("resolveKnowledgeImageUrls", () => {
  const convert = (path: string) => `asset://localhost/${encodeURIComponent(path)}`;

  it("converts wiki media paths to Tauri asset URLs", () => {
    const markdown = "![chart](media/report/img-1.png)";
    const result = resolveKnowledgeImageUrls(markdown, "/Users/me/.jkcodingagent/knowledge/collections/kc-1", convert);

    expect(result).toContain(
      "asset://localhost/%2FUsers%2Fme%2F.jkcodingagent%2Fknowledge%2Fcollections%2Fkc-1%2Fwiki%2Fmedia%2Freport%2Fimg-1.png",
    );
  });

  it("leaves http and data image URLs unchanged", () => {
    const markdown = "![a](https://example.com/a.png)\n![b](data:image/png;base64,aaa)";
    expect(resolveKnowledgeImageUrls(markdown, "/root", convert)).toBe(markdown);
  });
});
