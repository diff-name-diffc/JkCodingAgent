import {
  AcDbDatabaseConverterManager,
  AcDbFileType,
  acdbHostApplicationServices,
} from "@mlightcad/data-model";
import { AcApDocument, AcEdOpenMode } from "@mlightcad/cad-simple-viewer";
import libredwgParserWorkerUrl from "../../node_modules/.pnpm/node_modules/@mlightcad/libredwg-converter/dist/libredwg-parser-worker.js?url";

const DEFAULT_DWG_OPEN_ERROR = "cad-simple-viewer 无法打开该 DWG 文件";

let dwgSupportPromise: Promise<void> | null = null;

type OpenCadViewerDwgDocumentInput = {
  document: AcApDocument;
  fileName: string;
  content: ArrayBuffer;
  mode: AcEdOpenMode;
};

function buildProbeOptions(mode: AcEdOpenMode) {
  return {
    readOnly: mode === AcEdOpenMode.Read,
  };
}

function normalizeCadViewerOpenError(error: unknown): Error {
  const message = error instanceof Error ? error.message : String(error);

  if (message.includes("isn't registered")) {
    return new Error("DWG converter 未注册，cad-simple-viewer 当前缺少 DWG 解析器初始化。");
  }

  if (
    message.includes("worker") ||
    message.includes("Worker") ||
    message.includes("Failed to fetch") ||
    message.includes("importScripts")
  ) {
    return new Error(`DWG parser worker 加载失败：${message}`);
  }

  if (message.startsWith("DWG ")) {
    return new Error(message);
  }

  return new Error(`DWG 加载失败：${message}`);
}

async function loadLibreDwgConverter() {
  return import(
    // @ts-ignore The bundled dist entry is intentionally used for runtime compatibility.
    "../../node_modules/.pnpm/node_modules/@mlightcad/libredwg-converter/dist/libredwg-converter.js"
  );
}

async function registerDwgConverter() {
  const manager = AcDbDatabaseConverterManager.instance;
  const { AcDbLibreDwgConverter } = await loadLibreDwgConverter();
  manager.register(
    AcDbFileType.DWG,
    new AcDbLibreDwgConverter({
      convertByEntityType: false,
      useWorker: true,
      parserWorkerUrl: libredwgParserWorkerUrl,
    }),
  );
}

function activateWorkingDatabase(document: AcApDocument) {
  acdbHostApplicationServices().workingDatabase = document.database;
}

export async function ensureCadViewerDwgSupport() {
  if (!dwgSupportPromise) {
    dwgSupportPromise = registerDwgConverter().catch((error) => {
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
    activateWorkingDatabase(document);
    return;
  }

  await diagnoseDwgOpenFailure(fileName, content, mode);
}
