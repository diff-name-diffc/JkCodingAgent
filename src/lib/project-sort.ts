import type { Project } from "../types";

/**
 * 项目按最近打开降序（UI-10）。修复旧 railProjects 用 Number(id) 排序对
 * UUID 失效（NaN 比较）的遗留 bug；同时间戳回退到名称序保证稳定。
 */
export function sortProjectsByRecency(projects: Project[]): Project[] {
  return [...projects].sort(
    (a, b) => b.lastOpenedAt - a.lastOpenedAt || a.name.localeCompare(b.name, "zh-Hans-CN"),
  );
}
