import {
  AcDbBatchProcessing,
  AcDbBlockTableRecord,
  AcDbDatabaseConverterManager,
  AcDbFileType,
  AcDbLayerTableRecord,
  AcCmColor,
  acdbHostApplicationServices,
} from "@mlightcad/data-model";
import type {
  AcDbConversionProgressCallback,
  AcDbDatabase,
  AcDbDatabaseConverter,
  AcDbParsingTaskResult,
} from "@mlightcad/data-model";
import {
  AcApDocManager,
  AcApDocument,
  AcEdOpenMode,
  type AcApWebworkerFiles,
} from "@mlightcad/cad-simple-viewer";
import { Dwg_File_Type } from "@mlightcad/libredwg-web";
import type { DwgDatabase } from "@mlightcad/libredwg-web";
import { consumePreparedDwgViewerParseTask, ensureLibreDwg } from "./dwgSharedParse";
import dxfParserWorkerUrl from "../../node_modules/@mlightcad/data-model/dist/dxf-parser-worker.js?url";
import dwgParserWorkerUrl from "../../node_modules/@mlightcad/cad-simple-viewer/dist/libredwg-parser-worker.js?url";
import mtextRenderWorkerUrl from "../../node_modules/@mlightcad/cad-simple-viewer/dist/mtext-renderer-worker.js?url";

const DEFAULT_DWG_OPEN_ERROR = "cad-simple-viewer 无法打开该 DWG 文件";
const DEFAULT_CJK_FALLBACK_FONTS = ["simsun", "simhei", "simkai"];
const MAX_DWG_FAILURE_DETAILS = 8;
const CAD_DATA_BASE_URL = "https://mlightcad.gitlab.io/cad-data/";
const DEFAULT_LAYER_NAME = "0";
const DEFAULT_LAYER_LINETYPE = "Continuous";

let dwgSupportPromise: Promise<void> | null = null;
let cadFontSupportPromise: Promise<void> = Promise.resolve();
const loadedCadFonts = new Set<string>();
let libreDwgEntityConverterCtorPromise: Promise<DwgEntityConverterConstructor> | null = null;
let cadFontManagerConfigured = false;

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
type DwgEntityConverterConstructor = new () => DwgEntityConverter;
type DwgEntityConverter = {
  convert: (entity: DwgSourceEntity) => unknown | null;
};
type DwgSourceEntity =
  | DwgDatabase["entities"][number]
  | DwgDatabase["tables"]["BLOCK_RECORD"]["entries"][number]["entities"][number];
type ConvertedDwgEntity = {
  label: string;
  dbEntity: unknown;
};
type SafeEntityAppendStats = {
  appendedCount: number;
  skippedCount: number;
  failureMessages: string[];
};
type AppendEntityTarget = {
  appendEntity(entities: unknown[] | unknown): void;
};
type InternalCadDocManager = {
  registerWorkers?: (webworkerFileUrls?: AcApWebworkerFiles) => void;
};

const CAD_WEBWORKER_FILE_URLS: AcApWebworkerFiles = {
  dxfParser: dxfParserWorkerUrl,
  dwgParser: dwgParserWorkerUrl,
  mtextRender: mtextRenderWorkerUrl,
};

async function loadLibreDwgConverterConstructor(): Promise<LibreDwgConverterConstructor> {
  const module = await import(
    // @ts-expect-error 工程当前依赖布局下需直接走 dist 入口，避免包名解析失败。
    "../../node_modules/.pnpm/node_modules/@mlightcad/libredwg-converter/dist/libredwg-converter.js"
  );

  return module.AcDbLibreDwgConverter as LibreDwgConverterConstructor;
}

async function loadLibreDwgEntityConverterConstructor(): Promise<DwgEntityConverterConstructor> {
  if (!libreDwgEntityConverterCtorPromise) {
    libreDwgEntityConverterCtorPromise = import(
      // 工程当前依赖布局下需直接走 lib 入口，复用底层实体转换器。
      "../../node_modules/.pnpm/node_modules/@mlightcad/libredwg-converter/lib/AcDbEntitiyConverter.js"
    )
      .then((module) => module.AcDbEntityConverter as DwgEntityConverterConstructor)
      .catch((error) => {
        libreDwgEntityConverterCtorPromise = null;
        throw error;
      });
  }

  return libreDwgEntityConverterCtorPromise;
}

function groupDwgEntitiesByType<T extends DwgSourceEntity>(entities: readonly T[]): T[] {
  const grouped = new Map<string, T[]>();

  for (const entity of entities) {
    const type = typeof entity?.type === "string" ? entity.type : "UNKNOWN";
    const bucket = grouped.get(type);
    if (bucket) {
      bucket.push(entity);
      continue;
    }
    grouped.set(type, [entity]);
  }

  return Array.from(grouped.values()).flat();
}

function normalizeDwgLayerName(layerName: unknown) {
  return typeof layerName === "string" && layerName.trim().length > 0
    ? layerName.trim()
    : DEFAULT_LAYER_NAME;
}

function collectReferencedDwgLayerNames(model: DwgDatabase) {
  const names = new Set<string>([DEFAULT_LAYER_NAME]);
  model.tables.BLOCK_RECORD.entries.forEach((btr) => {
    btr.entities?.forEach((entity) => {
      names.add(normalizeDwgLayerName(entity?.layer));
    });
  });
  return names;
}

function ensureDwgLayerExists(db: AcDbDatabase, layerName: string) {
  if (db.tables.layerTable.getAt(layerName)) {
    return;
  }

  const color = new AcCmColor();
  color.colorIndex = 7;
  db.tables.layerTable.add(
    new AcDbLayerTableRecord({
      name: layerName,
      standardFlags: 0,
      linetype: DEFAULT_LAYER_LINETYPE,
      lineWeight: 0,
      isOff: false,
      color,
      isPlottable: true,
    }),
  );
}

function ensureReferencedDwgLayers(model: DwgDatabase, db: AcDbDatabase) {
  collectReferencedDwgLayerNames(model).forEach((layerName) => {
    ensureDwgLayerExists(db, layerName);
  });
}

function describeDwgEntity(entity: DwgSourceEntity, index: number) {
  const type = typeof entity?.type === "string" ? entity.type : "UNKNOWN";
  const handle =
    typeof entity?.handle === "string" || typeof entity?.handle === "number"
      ? String(entity.handle)
      : `#${index}`;
  return `${type}@${handle}`;
}

function pushFailureMessage(
  failureMessages: string[],
  entityLabel: string,
  error: unknown,
  stage: "convert" | "append",
) {
  if (failureMessages.length >= MAX_DWG_FAILURE_DETAILS) {
    return;
  }
  const verb = stage === "append" ? "追加失败" : "转换失败";
  failureMessages.push(`${entityLabel} ${verb}: ${describeCadViewerError(error)}`);
}

function appendConvertedEntities(
  target: AppendEntityTarget,
  converted: ConvertedDwgEntity[],
  failureMessages: string[],
): Pick<SafeEntityAppendStats, "appendedCount" | "skippedCount"> {
  if (converted.length === 0) {
    return {
      appendedCount: 0,
      skippedCount: 0,
    };
  }

  try {
    target.appendEntity(converted.map((entry) => entry.dbEntity));
    return {
      appendedCount: converted.length,
      skippedCount: 0,
    };
  } catch {
    let appendedCount = 0;
    let skippedCount = 0;

    for (const entry of converted) {
      try {
        target.appendEntity([entry.dbEntity]);
        appendedCount += 1;
      } catch (appendError) {
        skippedCount += 1;
        pushFailureMessage(failureMessages, entry.label, appendError, "append");
      }
    }

    return {
      appendedCount,
      skippedCount,
    };
  }
}

function appendDwgEntitySliceSafely(
  converter: DwgEntityConverter,
  target: AppendEntityTarget,
  entities: readonly DwgSourceEntity[],
  globalStartIndex: number,
): SafeEntityAppendStats {
  const failureMessages: string[] = [];

  try {
    const converted: ConvertedDwgEntity[] = [];
    entities.forEach((entity, index) => {
      const dbEntity = converter.convert(entity);
      if (dbEntity) {
        converted.push({
          label: describeDwgEntity(entity, globalStartIndex + index),
          dbEntity,
        });
      }
    });

    return {
      ...appendConvertedEntities(target, converted, failureMessages),
      failureMessages,
    };
  } catch {
    let appendedCount = 0;
    let skippedCount = 0;

    entities.forEach((entity, index) => {
      const label = describeDwgEntity(entity, globalStartIndex + index);
      try {
        const dbEntity = converter.convert(entity);
        if (!dbEntity) {
          return;
        }
        const appendStats = appendConvertedEntities(
          target,
          [{ label, dbEntity }],
          failureMessages,
        );
        appendedCount += appendStats.appendedCount;
        skippedCount += appendStats.skippedCount;
      } catch (error) {
        skippedCount += 1;
        pushFailureMessage(failureMessages, label, error, "convert");
      }
    });

    return {
      appendedCount,
      skippedCount,
      failureMessages,
    };
  }
}

async function appendDwgEntitiesInChunks({
  converter,
  target,
  entities,
  minimumChunkSize,
  startPercentage,
  progress,
  preserveEntityOrder,
}: {
  converter: DwgEntityConverter;
  target: AppendEntityTarget;
  entities: readonly DwgSourceEntity[];
  minimumChunkSize: number;
  startPercentage: { value: number };
  progress?: AcDbConversionProgressCallback;
  preserveEntityOrder: boolean;
}) {
  const sourceEntities = preserveEntityOrder ? [...entities] : groupDwgEntitiesByType(entities);
  const entityCount = sourceEntities.length;
  const batchProcessor = new AcDbBatchProcessing(
    entityCount,
    Math.max(1, 100 - startPercentage.value),
    minimumChunkSize,
  );
  let appendedCount = 0;
  let skippedCount = 0;
  const failureMessages: string[] = [];

  await batchProcessor.processChunk(async (start, end) => {
    const stats = appendDwgEntitySliceSafely(converter, target, sourceEntities.slice(start, end), start);
    appendedCount += stats.appendedCount;
    skippedCount += stats.skippedCount;
    failureMessages.push(...stats.failureMessages);

    if (progress) {
      let percentage =
        startPercentage.value + (end / Math.max(1, entityCount)) * (100 - startPercentage.value);
      if (percentage > 100) {
        percentage = 100;
      }
      await progress(percentage, "ENTITY", "IN-PROGRESS");
    }
  });

  return {
    appendedCount,
    skippedCount,
    failureMessages: failureMessages.slice(0, MAX_DWG_FAILURE_DETAILS),
  };
}

function warnOnSkippedDwgEntities(scope: string, stats: SafeEntityAppendStats) {
  if (stats.skippedCount <= 0) {
    return;
  }

  const detail =
    stats.failureMessages.length > 0
      ? ` 示例：${stats.failureMessages.join(" | ")}`
      : "";
  console.warn(
    `[cad-viewer] ${scope} 时跳过 ${stats.skippedCount} 个实体，已保留 ${stats.appendedCount} 个可渲染实体。${detail}`,
  );
}

async function createManagedDwgConverter() {
  const BaseLibreDwgConverter = await loadLibreDwgConverterConstructor();
  const EntityConverter = await loadLibreDwgEntityConverterConstructor();

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
        const wrappedError = new Error(`DWG 主线程解析失败：${describeCadViewerError(error)}`);
        (wrappedError as Error & { cause?: unknown }).cause = error;
        throw wrappedError;
      } finally {
        libredwg.dwg_free(handle);
      }
    }

    protected processBlockTables(model: DwgDatabase, db: AcDbDatabase): void {
      ensureReferencedDwgLayers(model, db);
      const btrs = model.tables.BLOCK_RECORD.entries;

      btrs.forEach((btr) => {
        let dbBlock = db.tables.blockTable.getAt(btr.name);
        if (!dbBlock) {
          dbBlock = new AcDbBlockTableRecord();
          dbBlock.objectId = btr.handle;
          dbBlock.name = btr.name;
          dbBlock.ownerId = btr.ownerHandle;
          dbBlock.origin.copy(btr.basePoint);
          dbBlock.layoutId = btr.layout;
          dbBlock.blockInsertUnits = btr.insertionUnits;
          dbBlock.explodability = btr.explodability;
          dbBlock.blockScaling = btr.scalability;
          if (btr.bmpPreview) {
            dbBlock.bmpPreview = btr.bmpPreview;
          }
          db.tables.blockTable.add(dbBlock);
        }

        if (!dbBlock.isModelSapce && btr.entities && btr.entities.length > 0) {
          const stats = appendDwgEntitySliceSafely(new EntityConverter(), dbBlock, btr.entities, 0);
          warnOnSkippedDwgEntities(`转换块 ${btr.name}`, stats);
        }
      });
    }

    protected async processEntities(
      model: DwgDatabase,
      db: AcDbDatabase,
      minimumChunkSize: number,
      startPercentage: { value: number },
      progress?: AcDbConversionProgressCallback,
    ): Promise<void> {
      let entities: DwgSourceEntity[] = [];
      model.tables.BLOCK_RECORD.entries.forEach((btr) => {
        if (btr.name === "*MODEL_SPACE") {
          entities = btr.entities;
        }
      });

      const stats = await appendDwgEntitiesInChunks({
        converter: new EntityConverter(),
        target: db.tables.blockTable.modelSpace,
        entities,
        minimumChunkSize,
        startPercentage,
        progress,
        preserveEntityOrder: this.config.convertByEntityType !== true,
      });
      warnOnSkippedDwgEntities("转换模型空间", stats);
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

function configureCadFontManager(manager: AcApDocManager) {
  if (cadFontManagerConfigured) {
    return manager;
  }

  (manager as unknown as InternalCadDocManager).registerWorkers?.(CAD_WEBWORKER_FILE_URLS);
  cadFontManagerConfigured = true;
  return manager;
}

function getCadFontManager() {
  const manager =
    AcApDocManager.createInstance({
      baseUrl: CAD_DATA_BASE_URL,
      notLoadDefaultFonts: true,
      webworkerFileUrls: CAD_WEBWORKER_FILE_URLS,
    }) ?? AcApDocManager.instance;
  return configureCadFontManager(manager);
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
