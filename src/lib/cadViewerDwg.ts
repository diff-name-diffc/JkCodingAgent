import {
  AcDbDatabaseConverterManager,
  AcDbFileType,
  acdbHostApplicationServices,
} from "@mlightcad/data-model";
import type { AcDbDatabaseConverter } from "@mlightcad/data-model";
import type { AcDbParsingTaskResult } from "@mlightcad/data-model";
import { AcApDocManager, AcApDocument, AcEdOpenMode } from "@mlightcad/cad-simple-viewer";
import { Dwg_File_Type } from "@mlightcad/libredwg-web";
import type { DwgDatabase } from "@mlightcad/libredwg-web";
import { consumePreparedDwgViewerParseTask, ensureLibreDwg } from "./dwgSharedParse";

const DEFAULT_DWG_OPEN_ERROR = "cad-simple-viewer 无法打开该 DWG 文件";
const DEFAULT_CJK_FALLBACK_FONTS = ["simsun", "simhei", "simkai"];

let dwgSupportPromise: Promise<void> | null = null;
let cadFontSupportPromise: Promise<void> = Promise.resolve();
const loadedCadFonts = new Set<string>();

type OpenCadViewerDwgDocumentInput = {
  document: AcApDocument;
  fileName: string;
  content: ArrayBuffer;
  mode: AcEdOpenMode;
};

function describeCadViewerError(error: unknown): string {
  if (error instanceof Error) {
    return error.message || error.stack || "未知 Error";
  }

  if (typeof error === "string") {
    return error;
  }

  if (error && typeof error === "object") {
    const message =
      "message" in error && typeof error.message === "string"
        ? error.message
        : "reason" in error && typeof error.reason === "string"
          ? error.reason
          : null;

    if (message) {
      return message;
    }

    try {
      return JSON.stringify(error);
    } catch {
      return Object.prototype.toString.call(error);
    }
  }

  if (typeof error === "undefined") {
    return "未知错误（undefined）";
  }

  return String(error);
}

function buildProbeOptions(mode: AcEdOpenMode) {
  return {
    readOnly: mode === AcEdOpenMode.Read,
  };
}

function normalizeCadViewerOpenError(error: unknown): Error {
  const message = describeCadViewerError(error);

  if (message.includes("isn't registered")) {
    return new Error("DWG converter 未注册，cad-simple-viewer 当前缺少 DWG 解析器初始化。");
  }

  if (
    message.includes("Failed to fetch dynamically imported module") ||
    message.includes("importScripts") ||
    message.includes("Worker error:") ||
    message.includes("worker")
  ) {
    return new Error(`DWG 解析器初始化失败：${message}`);
  }

  if (
    message.includes("DWG 数据句柄创建失败") ||
    message.includes("DWG 主线程解析失败") ||
    message.includes("Failed to parse drawing") ||
    message.includes("Failed to read dwg data")
  ) {
    return new Error(`DWG 文件解析失败：${message}`);
  }

  if (message.startsWith("DWG ")) {
    return new Error(message);
  }

  return new Error(`DWG 加载失败：${message}`);
}

type LibreDwgConverterConstructor = new (
  config?: Record<string, unknown>,
) => AcDbDatabaseConverter<DwgDatabase>;

async function loadLibreDwgConverterConstructor(): Promise<LibreDwgConverterConstructor> {
  const module = await import(
    // @ts-ignore 工程当前依赖布局下需直接走 dist 入口，避免包名解析失败。
    "../../node_modules/.pnpm/node_modules/@mlightcad/libredwg-converter/dist/libredwg-converter.js"
  );

  return module.AcDbLibreDwgConverter as LibreDwgConverterConstructor;
}

async function createManagedDwgConverter() {
  const BaseLibreDwgConverter = await loadLibreDwgConverterConstructor();

  return new (class ManagedLibreDwgConverter extends BaseLibreDwgConverter {
    protected async parse(data: ArrayBuffer): Promise<AcDbParsingTaskResult<DwgDatabase>> {
      const prepared = consumePreparedDwgViewerParseTask(data);
      if (prepared) {
        const resolved = await prepared;
        return {
          model: resolved.model,
          data: resolved.stats,
        };
      }

      const libredwg = await ensureLibreDwg();
      const handle = libredwg.dwg_read_data(data.slice(0), Dwg_File_Type.DWG);

      if (typeof handle !== "number") {
        throw new Error("DWG 数据句柄创建失败");
      }

      try {
        const converted = libredwg.convertEx(handle);
        if (!converted?.database) {
          throw new Error("DWG 解析结果缺少 database");
        }

        return {
          model: converted.database,
          data: converted.stats ?? { unknownEntityCount: 0 },
        };
      } catch (error) {
        throw new Error(`DWG 主线程解析失败：${describeCadViewerError(error)}`);
      } finally {
        libredwg.dwg_free(handle);
      }
    }
  })({
    convertByEntityType: false,
  });
}

async function registerDwgConverter() {
  const manager = AcDbDatabaseConverterManager.instance;
  manager.register(AcDbFileType.DWG, await createManagedDwgConverter());
}

function activateWorkingDatabase(document: AcApDocument) {
  acdbHostApplicationServices().workingDatabase = document.database;
}

function getCadFontManager() {
  return AcApDocManager.createInstance({ notLoadDefaultFonts: true }) ?? AcApDocManager.instance;
}

async function ensureCadViewerFontSupport(fontNames: string[]) {
  const requestedFonts = Array.from(
    new Set(
      [...fontNames, ...DEFAULT_CJK_FALLBACK_FONTS]
        .map((fontName) => fontName.trim())
        .filter((fontName) => fontName.length > 0),
    ),
  );

  if (requestedFonts.length === 0) {
    return;
  }

  const missingFonts = requestedFonts.filter(
    (fontName) => !loadedCadFonts.has(fontName.toLowerCase()),
  );

  if (missingFonts.length === 0) {
    return;
  }

  const loadPromise = cadFontSupportPromise.then(async () => {
    const nextMissingFonts = missingFonts.filter(
      (fontName) => !loadedCadFonts.has(fontName.toLowerCase()),
    );
    if (nextMissingFonts.length === 0) {
      return;
    }

    await getCadFontManager().loadDefaultFonts(nextMissingFonts);
    nextMissingFonts.forEach((fontName) => {
      loadedCadFonts.add(fontName.toLowerCase());
    });
  });

  cadFontSupportPromise = loadPromise.catch(() => undefined);
  await loadPromise;
}

export async function ensureCadViewerDwgSupport() {
  if (!dwgSupportPromise) {
    dwgSupportPromise = Promise.resolve()
      .then(() => registerDwgConverter())
      .catch((error) => {
        dwgSupportPromise = null;
        throw normalizeCadViewerOpenError(error);
      });
  }

  return dwgSupportPromise;
}

async function diagnoseDwgOpenFailure(fileName: string, content: ArrayBuffer, mode: AcEdOpenMode) {
  const probeDocument = new AcApDocument();
  activateWorkingDatabase(probeDocument);

  try {
    await probeDocument.database.read(content, buildProbeOptions(mode), AcDbFileType.DWG);
  } catch (error) {
    throw normalizeCadViewerOpenError(error);
  }

  throw new Error(`${DEFAULT_DWG_OPEN_ERROR}：${fileName}`);
}

export async function openCadViewerDwgDocument({
  document,
  fileName,
  content,
  mode,
}: OpenCadViewerDwgDocumentInput) {
  await ensureCadViewerDwgSupport();
  activateWorkingDatabase(document);

  const opened = await document.openDocument(fileName, content, { mode });
  if (opened) {
    const drawingFonts = document.database.tables.textStyleTable.fonts;
    await ensureCadViewerFontSupport(drawingFonts).catch((error) => {
      console.warn("[cad-viewer] 字体预加载失败，标注文字可能无法显示。", error);
    });
    activateWorkingDatabase(document);
    return;
  }

  await diagnoseDwgOpenFailure(fileName, content, mode);
}
