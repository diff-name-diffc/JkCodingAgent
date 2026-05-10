import { useEffect, useMemo, useRef, useState } from "react";
import type React from "react";
import { FileText, LoaderCircle, Network, X } from "lucide-react";
import * as THREE from "three";
import type { KnowledgeGraph, KnowledgeGraphNode, KnowledgePageContent } from "../../types";
import s from "../../styles";

type GraphPoint = {
  node: KnowledgeGraphNode;
  position: THREE.Vector3;
  color: string;
  radius: number;
};

type LabelPosition = {
  x: number;
  y: number;
  visible: boolean;
  scale: number;
};

const GRAPH_COLORS = ["#5d7cff", "#16a34a", "#d97706", "#dc2626", "#0ea5e9", "#a855f7", "#14b8a6"];
const EDGE_KEY_SEPARATOR = "\u0000";
const INITIAL_GRAPH_ROTATION = { x: -0.18, y: 0.5 };
const LABEL_WORLD_OFFSET = 3.1;
const CLICK_DRAG_THRESHOLD = 5;

export function KnowledgeGraphView({
  graph,
  selectedPath,
  selectedPage,
  pageLoading,
  onSelectPage,
  onClosePage,
  renderPageContent,
}: {
  graph: KnowledgeGraph | null;
  selectedPath: string | null;
  selectedPage: KnowledgePageContent | null;
  pageLoading: boolean;
  onSelectPage: (relativePath: string) => void;
  onClosePage: () => void;
  renderPageContent: (content: string) => React.ReactNode;
}) {
  if (!graph) {
    return (
      <div style={s.knowledgePanel}>
        <div style={s.knowledgeGraphEmpty}>
          <Network size={34} />
          <span>选择集合后可生成图谱。</span>
        </div>
      </div>
    );
  }

  if (graph.nodes.length === 0) {
    return (
      <div style={s.knowledgePanel}>
        <div style={s.knowledgeGraphEmpty}>
          <Network size={34} />
          <span>暂无 Wiki 页面。</span>
        </div>
      </div>
    );
  }

  return (
    <GraphScene
      graph={graph}
      selectedPath={selectedPath}
      selectedPage={selectedPage}
      pageLoading={pageLoading}
      onSelectPage={onSelectPage}
      onClosePage={onClosePage}
      renderPageContent={renderPageContent}
    />
  );
}

function GraphScene({
  graph,
  selectedPath,
  selectedPage,
  pageLoading,
  onSelectPage,
  onClosePage,
  renderPageContent,
}: {
  graph: KnowledgeGraph;
  selectedPath: string | null;
  selectedPage: KnowledgePageContent | null;
  pageLoading: boolean;
  onSelectPage: (relativePath: string) => void;
  onClosePage: () => void;
  renderPageContent: (content: string) => React.ReactNode;
}) {
  const canvasHostRef = useRef<HTMLDivElement | null>(null);
  const rotationRef = useRef({ ...INITIAL_GRAPH_ROTATION });
  const [renderError, setRenderError] = useState<string | null>(null);
  const [labelPositions, setLabelPositions] = useState<Map<string, LabelPosition>>(() => new Map());

  const selectedNode = useMemo(
    () => graph.nodes.find((node) => node.path === selectedPath) ?? null,
    [graph.nodes, selectedPath],
  );
  const { relatedIds, selectedEdgeKeys } = useMemo(
    () => resolveSelection(graph, selectedNode?.id ?? null),
    [graph, selectedNode],
  );
  const points = useMemo(() => createGraphPoints(graph), [graph]);

  useEffect(() => {
    const host = canvasHostRef.current;
    if (!host) return;

    let frameId = 0;
    let disposed = false;
    let hoveredMesh: THREE.Mesh | null = null;
    let dragState: {
      pointerId: number;
      startX: number;
      startY: number;
      startRotationX: number;
      startRotationY: number;
      moved: boolean;
    } | null = null;
    setRenderError(null);

    const scene = new THREE.Scene();
    scene.background = new THREE.Color(0x101418);
    scene.fog = new THREE.Fog(0x101418, 42, 96);

    const camera = new THREE.PerspectiveCamera(46, 1, 0.1, 160);
    camera.position.set(0, 18, 46);
    camera.lookAt(0, 0, 0);

    let renderer: THREE.WebGLRenderer;
    try {
      renderer = new THREE.WebGLRenderer({ antialias: true, alpha: false });
    } catch (error) {
      setRenderError(`3D 渲染初始化失败：${String(error)}`);
      return;
    }

    renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
    renderer.setClearColor(0x101418, 1);
    renderer.domElement.style.pointerEvents = "none";
    host.appendChild(renderer.domElement);

    const graphGroup = new THREE.Group();
    graphGroup.rotation.set(rotationRef.current.x, rotationRef.current.y, 0);
    scene.add(graphGroup);
    scene.add(new THREE.AmbientLight(0xffffff, 1.7));

    const keyLight = new THREE.PointLight(0x8aa2ff, 32, 120);
    keyLight.position.set(-18, 24, 28);
    scene.add(keyLight);

    const fillLight = new THREE.PointLight(0x4ade80, 14, 90);
    fillLight.position.set(24, -12, -18);
    scene.add(fillLight);

    const grid = new THREE.GridHelper(64, 24, 0x2d3946, 0x202932);
    grid.position.y = -11;
    graphGroup.add(grid);

    const meshes: THREE.Mesh[] = [];
    const pointById = new Map(points.map((point) => [point.node.id, point]));
    for (const point of points) {
      const isDimmed = relatedIds.size > 0 && !relatedIds.has(point.node.id);
      const isSelected = selectedNode?.id === point.node.id;
      const geometry = new THREE.SphereGeometry(point.radius * (isSelected ? 1.28 : 1), 32, 24);
      const material = new THREE.MeshStandardMaterial({
        color: point.color,
        emissive: point.color,
        emissiveIntensity: isSelected ? 0.9 : isDimmed ? 0.06 : 0.34,
        metalness: 0.24,
        roughness: 0.42,
        transparent: true,
        opacity: isDimmed ? 0.28 : 0.96,
      });
      const mesh = new THREE.Mesh(geometry, material);
      mesh.position.copy(point.position);
      mesh.userData = { nodeId: point.node.id, path: point.node.path };
      meshes.push(mesh);
      graphGroup.add(mesh);
    }

    for (const edge of graph.edges) {
      const source = pointById.get(edge.source);
      const target = pointById.get(edge.target);
      if (!source || !target) continue;

      const edgeKey = makeEdgeKey(edge.source, edge.target);
      const isRelated = selectedEdgeKeys.has(edgeKey);
      const isDimmed = selectedEdgeKeys.size > 0 && !isRelated;
      const geometry = new THREE.BufferGeometry().setFromPoints([source.position, target.position]);
      const material = new THREE.LineBasicMaterial({
        color: isRelated ? 0xffffff : 0x8090a8,
        transparent: true,
        opacity: isRelated ? 0.92 : isDimmed ? 0.08 : Math.min(0.2 + edge.weight * 0.08, 0.58),
      });
      const line = new THREE.Line(geometry, material);
      graphGroup.add(line);
    }

    const raycaster = new THREE.Raycaster();
    const pointer = new THREE.Vector2();

    const resize = () => {
      const width = Math.max(host.clientWidth, 320);
      const height = Math.max(host.clientHeight, 320);
      renderer.setSize(width, height, false);
      camera.aspect = width / height;
      camera.updateProjectionMatrix();
      queueRender();
    };

    const setPointerFromEvent = (event: PointerEvent) => {
      const rect = host.getBoundingClientRect();
      pointer.x = ((event.clientX - rect.left) / rect.width) * 2 - 1;
      pointer.y = -((event.clientY - rect.top) / rect.height) * 2 + 1;
    };

    const pickNode = (event: PointerEvent) => {
      setPointerFromEvent(event);
      raycaster.setFromCamera(pointer, camera);
      return raycaster.intersectObjects(meshes, false)[0]?.object as THREE.Mesh | undefined;
    };

    const handlePointerMove = (event: PointerEvent) => {
      if (dragState) {
        const deltaX = event.clientX - dragState.startX;
        const deltaY = event.clientY - dragState.startY;
        dragState.moved = dragState.moved || Math.hypot(deltaX, deltaY) > CLICK_DRAG_THRESHOLD;
        graphGroup.rotation.y = dragState.startRotationY + deltaX * 0.008;
        graphGroup.rotation.x = THREE.MathUtils.clamp(
          dragState.startRotationX + deltaY * 0.006,
          -1.15,
          0.85,
        );
        rotationRef.current = { x: graphGroup.rotation.x, y: graphGroup.rotation.y };
        host.style.cursor = "grabbing";
        queueRender();
        return;
      }

      const nextHover = pickNode(event) ?? null;
      if (hoveredMesh === nextHover) return;
      hoveredMesh = nextHover;
      host.style.cursor = hoveredMesh ? "pointer" : "grab";
    };

    const handlePointerDown = (event: PointerEvent) => {
      dragState = {
        pointerId: event.pointerId,
        startX: event.clientX,
        startY: event.clientY,
        startRotationX: graphGroup.rotation.x,
        startRotationY: graphGroup.rotation.y,
        moved: false,
      };
      host.setPointerCapture(event.pointerId);
      host.style.cursor = "grabbing";
    };

    const releasePointer = (pointerId: number) => {
      if (host.hasPointerCapture(pointerId)) host.releasePointerCapture(pointerId);
    };

    const handlePointerUp = (event: PointerEvent) => {
      host.style.cursor = hoveredMesh ? "pointer" : "grab";
      if (dragState?.pointerId === event.pointerId) {
        releasePointer(event.pointerId);
        if (!dragState.moved) {
          const mesh = pickNode(event);
          const path = mesh?.userData.path;
          if (typeof path === "string") onSelectPage(path);
        }
        dragState = null;
      }
    };

    const handlePointerCancel = (event: PointerEvent) => {
      if (dragState?.pointerId === event.pointerId) {
        releasePointer(event.pointerId);
        dragState = null;
      }
      host.style.cursor = hoveredMesh ? "pointer" : "grab";
    };

    const renderFrame = () => {
      if (disposed) return;
      frameId = 0;
      const width = renderer.domElement.clientWidth;
      const height = renderer.domElement.clientHeight;
      const nextLabelPositions = new Map<string, LabelPosition>();
      graphGroup.updateMatrixWorld(true);
      for (const point of points) {
        const worldPoint = point.position
          .clone()
          .setY(point.position.y + point.radius + LABEL_WORLD_OFFSET);
        worldPoint.applyMatrix4(graphGroup.matrixWorld).project(camera);
        nextLabelPositions.set(point.node.id, {
          x: (worldPoint.x * 0.5 + 0.5) * width,
          y: (-worldPoint.y * 0.5 + 0.5) * height,
          visible: worldPoint.z < 1,
          scale: THREE.MathUtils.clamp(1.08 - worldPoint.z * 0.24, 0.74, 1.18),
        });
      }
      setLabelPositions(nextLabelPositions);

      renderer.render(scene, camera);
    };

    const queueRender = () => {
      if (frameId !== 0) return;
      frameId = window.requestAnimationFrame(renderFrame);
    };

    const observer = new ResizeObserver(resize);
    observer.observe(host);
    resize();
    queueRender();
    host.style.cursor = "grab";
    host.addEventListener("pointermove", handlePointerMove);
    host.addEventListener("pointerdown", handlePointerDown);
    host.addEventListener("pointerup", handlePointerUp);
    host.addEventListener("pointercancel", handlePointerCancel);
    return () => {
      disposed = true;
      if (frameId !== 0) window.cancelAnimationFrame(frameId);
      observer.disconnect();
      host.removeEventListener("pointermove", handlePointerMove);
      host.removeEventListener("pointerdown", handlePointerDown);
      host.removeEventListener("pointerup", handlePointerUp);
      host.removeEventListener("pointercancel", handlePointerCancel);
      host.style.cursor = "";
      renderer.dispose();
      scene.traverse((object) => {
        if (object instanceof THREE.Mesh || object instanceof THREE.Line) {
          object.geometry.dispose();
          const material = object.material;
          if (Array.isArray(material)) material.forEach((item) => item.dispose());
          else material.dispose();
        }
      });
      renderer.domElement.remove();
    };
  }, [graph, onSelectPage, points, relatedIds, selectedEdgeKeys, selectedNode]);

  return (
    <div style={s.knowledgeGraphShell}>
      <div style={s.knowledgeGraphCanvasWrap}>
        <div style={s.knowledgeGraphHeader}>
          <div>
            <div style={s.knowledgeGraphTitle}>3D 知识图谱</div>
            <div style={s.knowledgeGraphMeta}>
              {graph.nodes.length} 节点 · {graph.edges.length} 关系
              {selectedNode ? ` · 已聚焦 ${selectedNode.label}` : ""}
            </div>
          </div>
        </div>
        <div ref={canvasHostRef} style={s.knowledgeGraphCanvas} aria-label="3D 知识图谱" />
        {renderError ? <div style={s.knowledgeGraphError}>{renderError}</div> : null}
        {points.map((point) => {
          const selected = selectedNode?.id === point.node.id;
          const related = relatedIds.size === 0 || relatedIds.has(point.node.id);
          const labelPosition = labelPositions.get(point.node.id);
          return (
            <button
              key={point.node.id}
              type="button"
              title={point.node.path}
              style={{
                ...s.knowledgeGraphLabel,
                left: labelPosition?.x ?? 0,
                top: labelPosition?.y ?? 0,
                opacity: labelPosition?.visible ? 1 : 0,
                transform: `translate(-50%, -100%) scale(${labelPosition?.scale ?? 1})`,
                borderColor: selected ? "rgba(255,255,255,0.72)" : "rgba(255,255,255,0.16)",
                background: selected ? "rgba(255,255,255,0.18)" : "rgba(10,14,18,0.58)",
                color: related ? "#f7fbff" : "rgba(247,251,255,0.42)",
              }}
              onPointerDown={(event) => {
                event.stopPropagation();
                onSelectPage(point.node.path);
              }}
              onClick={() => onSelectPage(point.node.path)}
            >
              <span style={{ ...s.knowledgeGraphLabelDot, background: point.color }} />
              <span style={s.knowledgeGraphLabelText}>{point.node.label}</span>
            </button>
          );
        })}
      </div>

      {selectedPath ? (
        <aside style={s.knowledgeGraphDrawer}>
          <div style={s.knowledgeGraphDrawerHeader}>
            <div style={{ minWidth: 0 }}>
              <div style={s.knowledgeGraphDrawerTitle}>
                <FileText size={15} />
                {selectedPage?.title ?? selectedNode?.label ?? "页面内容"}
              </div>
              <div style={s.knowledgeGraphDrawerPath}>{selectedPath}</div>
            </div>
            <button
              type="button"
              style={s.knowledgeIconBtn}
              onClick={onClosePage}
              aria-label="关闭图谱抽屉"
            >
              <X size={15} />
            </button>
          </div>
          <div style={s.knowledgeGraphDrawerBody}>
            {pageLoading ? (
              <div style={s.knowledgeGraphDrawerState}>
                <LoaderCircle size={18} className="spin" />
                正在读取页面...
              </div>
            ) : selectedPage ? (
              renderPageContent(selectedPage.content)
            ) : (
              <div style={s.knowledgeGraphDrawerState}>未找到对应页面。</div>
            )}
          </div>
        </aside>
      ) : null}
    </div>
  );
}

function createGraphPoints(graph: KnowledgeGraph): GraphPoint[] {
  const degreeById = new Map<string, number>();
  for (const node of graph.nodes) degreeById.set(node.id, 0);
  for (const edge of graph.edges) {
    degreeById.set(edge.source, (degreeById.get(edge.source) ?? 0) + edge.weight);
    degreeById.set(edge.target, (degreeById.get(edge.target) ?? 0) + edge.weight);
  }

  const typeByNode = new Map<string, KnowledgeGraphNode[]>();
  for (const node of graph.nodes) {
    const group = typeByNode.get(node.pageType) ?? [];
    group.push(node);
    typeByNode.set(node.pageType, group);
  }

  const types = [...typeByNode.keys()].sort((a, b) => a.localeCompare(b));
  const colorByType = new Map(
    types.map((type, index) => [type, GRAPH_COLORS[index % GRAPH_COLORS.length]]),
  );

  return types.flatMap((type, typeIndex) => {
    const nodes = typeByNode.get(type) ?? [];
    const typeAngle = (Math.PI * 2 * typeIndex) / Math.max(types.length, 1);
    const clusterX = Math.cos(typeAngle) * 14;
    const clusterZ = Math.sin(typeAngle) * 14;

    return nodes.map((node, index) => {
      const localAngle = (Math.PI * 2 * index) / Math.max(nodes.length, 1);
      const localRadius = 4.8 + (index % 3) * 1.9;
      const degree = degreeById.get(node.id) ?? 0;
      return {
        node,
        color: colorByType.get(type) ?? GRAPH_COLORS[0],
        radius: THREE.MathUtils.clamp(0.75 + degree * 0.06, 0.75, 1.65),
        position: new THREE.Vector3(
          clusterX + Math.cos(localAngle) * localRadius,
          Math.sin(localAngle * 1.7) * 4.2 + THREE.MathUtils.clamp(degree * 0.08, 0, 4),
          clusterZ + Math.sin(localAngle) * localRadius,
        ),
      };
    });
  });
}

function resolveSelection(graph: KnowledgeGraph, selectedId: string | null) {
  const relatedIds = new Set<string>();
  const selectedEdgeKeys = new Set<string>();
  if (!selectedId) return { relatedIds, selectedEdgeKeys };

  relatedIds.add(selectedId);
  for (const edge of graph.edges) {
    if (edge.source !== selectedId && edge.target !== selectedId) continue;
    relatedIds.add(edge.source);
    relatedIds.add(edge.target);
    selectedEdgeKeys.add(makeEdgeKey(edge.source, edge.target));
  }
  return { relatedIds, selectedEdgeKeys };
}

function makeEdgeKey(source: string, target: string) {
  return `${source}${EDGE_KEY_SEPARATOR}${target}`;
}
