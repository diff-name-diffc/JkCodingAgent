import type React from "react";

import { common } from "./common";
import { dialogs } from "./dialogs";
import { layout } from "./layout";
import { subAgent } from "./subAgent";
import { task } from "./task";

const s = {
  ...layout,
  ...subAgent,
  ...dialogs,
  ...task,
  ...common,
} satisfies Record<string, React.CSSProperties>;

export default s;
