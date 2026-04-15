import { describe, expect, it } from "vitest";
import {
  loadTreeNodes,
  isSystemGroupNode,
  SYSTEM_CONFIG_GROUP_LABEL,
  type FsEntry,
  type TreeNode,
} from "../components/file-explorer/tree";

function createDir(name: string, path: string): FsEntry {
  return {
    name,
    path,
    is_dir: true,
  };
}

describe("file explorer tree helpers", () => {
  it("将根目录下的点开头文件夹聚合到系统配置分组中", async () => {
    const projectPath = "/repo";
    const readEntries = async (path: string): Promise<FsEntry[] | null> => {
      if (path !== projectPath) return [];
      return [
        createDir(".codex", "/repo/.codex"),
        createDir(".vscode", "/repo/.vscode"),
        createDir("src", "/repo/src"),
      ];
    };

    const nodes = await loadTreeNodes({
      path: projectPath,
      rootPath: projectPath,
      previousNodes: [],
      readEntries,
    });

    expect(nodes).not.toBeNull();
    expect(nodes).toHaveLength(2);
    expect(nodes?.[0].name).toBe("src");
    expect(nodes?.[1].name).toBe(SYSTEM_CONFIG_GROUP_LABEL);
    expect(isSystemGroupNode(nodes?.[1] as TreeNode)).toBe(true);
    expect(nodes?.[1].children?.map((node) => node.name)).toEqual([".codex", ".vscode"]);
  });

  it("刷新时保留系统配置分组及其子目录的展开状态", async () => {
    const projectPath = "/repo";
    const readEntries = async (path: string): Promise<FsEntry[] | null> => {
      if (path === projectPath) {
        return [createDir(".vscode", "/repo/.vscode")];
      }

      if (path === "/repo/.vscode") {
        return [createDir("settings", "/repo/.vscode/settings")];
      }

      return [];
    };

    const previousNodes: TreeNode[] = [
      {
        name: SYSTEM_CONFIG_GROUP_LABEL,
        path: "__nezha_system_config__:/repo",
        is_dir: true,
        expanded: true,
        kind: "system-group",
        iconName: ".config",
        children: [
          {
            name: ".vscode",
            path: "/repo/.vscode",
            is_dir: true,
            expanded: true,
            kind: "entry",
            children: [
              {
                name: "settings",
                path: "/repo/.vscode/settings",
                is_dir: true,
                expanded: false,
                kind: "entry",
                children: null,
              },
            ],
          },
        ],
      },
    ];

    const nodes = await loadTreeNodes({
      path: projectPath,
      rootPath: projectPath,
      previousNodes,
      readEntries,
    });

    const group = nodes?.[0];
    expect(group).toBeDefined();
    expect(isSystemGroupNode(group as TreeNode)).toBe(true);
    expect(group?.expanded).toBe(true);
    expect(group?.children?.[0].expanded).toBe(true);
    expect(group?.children?.[0].children?.[0].name).toBe("settings");
  });
});
