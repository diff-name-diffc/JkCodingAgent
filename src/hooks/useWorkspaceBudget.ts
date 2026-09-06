import { useEffect, useMemo, useState } from "react";
import {
  resolveWorkspaceBudget,
  type WorkspaceBudget,
  type WorkspaceBudgetInput,
} from "../components/project/workspace-budget";

type BudgetPrefs = Omit<WorkspaceBudgetInput, "viewportWidth" | "viewportHeight">;

/**
 * 视口尺寸跟踪（rAF 合帧）+ 偏好 → 每帧空间预算。
 * 偏好是冻结输入：窄窗的临时适配只体现在预算输出与 degradations 中，
 * 调用方不得把适配结果写回偏好（见 workspace-budget.ts 头注释）。
 */
export function useWorkspaceBudget(prefs: BudgetPrefs): WorkspaceBudget {
  const [viewport, setViewport] = useState(() => ({
    width: typeof window === "undefined" ? 1280 : window.innerWidth,
    height: typeof window === "undefined" ? 800 : window.innerHeight,
  }));

  useEffect(() => {
    let frame = 0;
    const onResize = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() =>
        setViewport({ width: window.innerWidth, height: window.innerHeight }),
      );
    };
    window.addEventListener("resize", onResize);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("resize", onResize);
    };
  }, []);

  return useMemo(
    () =>
      resolveWorkspaceBudget({
        ...prefs,
        viewportWidth: viewport.width,
        viewportHeight: viewport.height,
      }),
    // prefs 由各偏好原始值组成，调用方每次渲染新建对象；计算是纯算术，成本可忽略。
    [prefs, viewport.width, viewport.height],
  );
}
