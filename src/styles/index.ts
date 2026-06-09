import type React from "react";

import { appSettings } from "./appSettings";
import { common } from "./common";
import { dialogs } from "./dialogs";
import { layout } from "./layout";
import { knowledge } from "./knowledge";
import { panels } from "./panels";
import { subAgent } from "./subAgent";
import { task } from "./task";
import { terminal } from "./terminal";

const s = {
  ...appSettings,
  ...layout,
  ...knowledge,
  ...panels,
  ...subAgent,
  ...terminal,
  ...dialogs,
  ...task,
  ...common,
} satisfies Record<string, React.CSSProperties>;

export default s;
