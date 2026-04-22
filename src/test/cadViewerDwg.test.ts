import { beforeEach, describe, expect, it, vi } from "vitest";

const registerMock = vi.fn();
const libreDwgCreateMock = vi.fn();
const hostServices = { workingDatabase: null as unknown };

vi.mock("@mlightcad/data-model", () => ({
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
});
