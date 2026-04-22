import { beforeEach, describe, expect, it, vi } from "vitest";

const entityConvertMock = vi.fn();
const registerMock = vi.fn();
const libreDwgCreateMock = vi.fn();
const hostServices = { workingDatabase: null as unknown };

vi.mock("@mlightcad/data-model", () => ({
  AcDbBatchProcessing: class {
    count: number;
    chunkSize: number;

    constructor(count: number, numerOfChunk: number, minimumChunkSize: number) {
      this.count = count;
      this.chunkSize = Math.max(minimumChunkSize, Math.ceil(count / Math.max(1, numerOfChunk)));
    }

    async processChunk(callback: (start: number, end: number) => Promise<void>) {
      for (let start = 0; start < this.count; start += this.chunkSize) {
        await callback(start, Math.min(this.count, start + this.chunkSize));
      }
    }
  },
  AcDbBlockTableRecord: class {
    name = "";
    objectId = "";
    ownerId = "";
    layoutId = "";
    blockInsertUnits = 0;
    explodability = 0;
    blockScaling = 0;
    bmpPreview: unknown = null;
    isModelSapce = false;
    origin = {
      copy: vi.fn(),
    };
    appended: unknown[] = [];

    appendEntity(entities: unknown[]) {
      this.appended.push(...entities);
    }
  },
  AcDbDatabaseConverterManager: {
    instance: {
      register: registerMock,
    },
  },
  AcDbFileType: {
    DWG: "dwg",
  },
  acdbHostApplicationServices: () => hostServices,
}));

vi.mock(
  "../../node_modules/.pnpm/node_modules/@mlightcad/libredwg-converter/lib/AcDbEntitiyConverter.js",
  () => ({
    AcDbEntityConverter: class {
      convert(entity: unknown) {
        return entityConvertMock(entity);
      }
    },
  }),
);

vi.mock("@mlightcad/libredwg-web", () => ({
  Dwg_File_Type: {
    DWG: "dwg",
  },
  LibreDwg: {
    create: libreDwgCreateMock,
  },
}));

vi.mock("@mlightcad/cad-simple-viewer", () => ({
  AcApDocManager: {
    createInstance: () => null,
    instance: {
      loadDefaultFonts: vi.fn(),
    },
  },
  AcApDocument: class {},
  AcEdOpenMode: {
    Read: 0,
    Write: 1,
  },
}));

describe("ensureCadViewerDwgSupport", () => {
  beforeEach(() => {
    vi.resetModules();
    entityConvertMock.mockReset();
    registerMock.mockReset();
    libreDwgCreateMock.mockReset();
    hostServices.workingDatabase = null;
  });

  it("注册自管 DWG converter，并通过 libredwg 主线程解析图纸", async () => {
    const libredwg = {
      dwg_read_data: vi.fn(() => 101),
      convertEx: vi.fn(() => ({
        database: { entities: [] },
        stats: { unknownEntityCount: 2 },
      })),
      dwg_free: vi.fn(),
    };
    libreDwgCreateMock.mockResolvedValue(libredwg);

    const { ensureCadViewerDwgSupport } = await import("../lib/cadViewerDwg");

    await ensureCadViewerDwgSupport();

    expect(registerMock).toHaveBeenCalledTimes(1);

    const registeredConverter = registerMock.mock.calls[0]?.[1] as {
      parse: (data: ArrayBuffer) => Promise<unknown>;
    };
    const result = await registeredConverter.parse(new ArrayBuffer(8));

    expect(libreDwgCreateMock).toHaveBeenCalledTimes(1);
    expect(libredwg.dwg_read_data).toHaveBeenCalledTimes(1);
    expect(libredwg.convertEx).toHaveBeenCalledWith(101);
    expect(libredwg.dwg_free).toHaveBeenCalledWith(101);
    expect(result).toEqual({
      model: { entities: [] },
      data: { unknownEntityCount: 2 },
    });
  });

  it("命中预解析结果时复用已解析模型，避免重复调用 libredwg", async () => {
    const { ensureCadViewerDwgSupport } = await import("../lib/cadViewerDwg");
    const { registerPreparedDwgViewerParse } = await import("../lib/dwgSharedParse");
    const content = new ArrayBuffer(16);

    registerPreparedDwgViewerParse(content, {
      model: { entities: [{ id: "cached" }] } as never,
      stats: { unknownEntityCount: 0 },
    });

    await ensureCadViewerDwgSupport();

    const registeredConverter = registerMock.mock.calls[0]?.[1] as {
      parse: (data: ArrayBuffer) => Promise<unknown>;
    };
    const result = await registeredConverter.parse(content);

    expect(libreDwgCreateMock).not.toHaveBeenCalled();
    expect(result).toEqual({
      model: { entities: [{ id: "cached" }] },
      data: { unknownEntityCount: 0 },
    });
  });

  it("模型空间某个实体转换失败时，仍继续保留同批次其余可渲染实体", async () => {
    entityConvertMock.mockImplementation((entity: { handle: string }) => {
      if (entity.handle === "bad") {
        throw new Error("broken");
      }
      return { id: entity.handle };
    });

    const { ensureCadViewerDwgSupport } = await import("../lib/cadViewerDwg");

    await ensureCadViewerDwgSupport();

    const registeredConverter = registerMock.mock.calls[0]?.[1] as {
      processEntities: (
        model: {
          tables: {
            BLOCK_RECORD: {
              entries: Array<{ name: string; entities: Array<{ handle: string; type: string }> }>;
            };
          };
        },
        db: {
          tables: {
            blockTable: {
              modelSpace: {
                appendEntity: (entities: Array<{ id: string }>) => void;
              };
            };
          };
        },
        minimumChunkSize: number,
        startPercentage: { value: number },
      ) => Promise<void>;
    };

    const appendedIds: string[] = [];
    await registeredConverter.processEntities(
      {
        tables: {
          BLOCK_RECORD: {
            entries: [
              {
                name: "*MODEL_SPACE",
                entities: [
                  { handle: "1", type: "LINE" },
                  { handle: "bad", type: "LINE" },
                  { handle: "3", type: "LINE" },
                ],
              },
            ],
          },
        },
      },
      {
        tables: {
          blockTable: {
            modelSpace: {
              appendEntity: (entities) => {
                appendedIds.push(...entities.map((entity) => entity.id));
              },
            },
          },
        },
      },
      2,
      { value: 10 },
    );

    expect(appendedIds).toEqual(["1", "3"]);
  });

  it("块定义中的坏实体不会拖掉同块其余可渲染实体", async () => {
    entityConvertMock.mockImplementation((entity: { handle: string }) => {
      if (entity.handle === "bad-block") {
        throw new Error("broken-block");
      }
      return { id: entity.handle };
    });

    const { ensureCadViewerDwgSupport } = await import("../lib/cadViewerDwg");

    await ensureCadViewerDwgSupport();

    const registeredConverter = registerMock.mock.calls[0]?.[1] as {
      processBlockTables: (
        model: {
          tables: {
            BLOCK_RECORD: {
              entries: Array<{
                name: string;
                handle: string;
                ownerHandle: string;
                basePoint: unknown;
                layout: string;
                insertionUnits: number;
                explodability: number;
                scalability: number;
                bmpPreview?: unknown;
                entities: Array<{ handle: string; type: string }>;
              }>;
            };
          };
        },
        db: {
          tables: {
            blockTable: {
              getAt: (name: string) => null;
              add: (block: { name: string; appended: Array<{ id: string }> }) => void;
            };
          };
        },
      ) => void;
    };

    const addedBlocks: Array<{ name: string; appended: Array<{ id: string }> }> = [];
    registeredConverter.processBlockTables(
      {
        tables: {
          BLOCK_RECORD: {
            entries: [
              {
                name: "BLOCK_A",
                handle: "block-a",
                ownerHandle: "owner-a",
                basePoint: { x: 0, y: 0, z: 0 },
                layout: "layout-a",
                insertionUnits: 0,
                explodability: 0,
                scalability: 0,
                entities: [
                  { handle: "10", type: "LINE" },
                  { handle: "bad-block", type: "LINE" },
                  { handle: "30", type: "LINE" },
                ],
              },
            ],
          },
        },
      },
      {
        tables: {
          blockTable: {
            getAt: () => null,
            add: (block) => {
              addedBlocks.push(block);
            },
          },
        },
      },
    );

    expect(addedBlocks).toHaveLength(1);
    expect(addedBlocks[0]?.appended.map((entity) => entity.id)).toEqual(["10", "30"]);
  });
});
