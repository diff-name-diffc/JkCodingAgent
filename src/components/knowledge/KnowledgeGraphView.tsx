import type { KnowledgeGraph } from "../../types";
import s from "../../styles";

export function KnowledgeGraphView({
  graph,
  onOpenPage,
}: {
  graph: KnowledgeGraph | null;
  onOpenPage: (relativePath: string) => void;
}) {
  if (!graph) {
    return (
      <div style={s.knowledgePanel}>
        <div style={{ color: "var(--text-muted)", fontSize: 13 }}>选择集合后可生成图谱。</div>
      </div>
    );
  }

  if (graph.nodes.length === 0) {
    return (
      <div style={s.knowledgePanel}>
        <div style={{ color: "var(--text-muted)", fontSize: 13 }}>暂无 Wiki 页面。</div>
      </div>
    );
  }

  const width = 980;
  const height = 620;
  const centerX = width / 2;
  const centerY = height / 2;
  const radius = Math.min(width, height) * 0.36;
  const positions = new Map(
    graph.nodes.map((node, index) => {
      const angle = (Math.PI * 2 * index) / Math.max(graph.nodes.length, 1) - Math.PI / 2;
      return [
        node.id,
        {
          x: centerX + Math.cos(angle) * radius,
          y: centerY + Math.sin(angle) * radius,
        },
      ];
    }),
  );

  return (
    <div style={s.knowledgePanel}>
      <div style={{ ...s.knowledgeCard, height: "100%", minHeight: 680, overflow: "auto" }}>
        <svg width={width} height={height} role="img" aria-label="知识图谱">
          <rect width={width} height={height} fill="var(--bg-card)" rx={8} />
          {graph.edges.map((edge) => {
            const source = positions.get(edge.source);
            const target = positions.get(edge.target);
            if (!source || !target) return null;
            return (
              <line
                key={`${edge.source}-${edge.target}`}
                x1={source.x}
                y1={source.y}
                x2={target.x}
                y2={target.y}
                stroke="var(--border-strong)"
                strokeOpacity={Math.min(0.18 + edge.weight * 0.08, 0.65)}
                strokeWidth={Math.min(1 + edge.weight * 0.4, 5)}
              />
            );
          })}
          {graph.nodes.map((node) => {
            const pos = positions.get(node.id);
            if (!pos) return null;
            return (
              <g
                key={node.id}
                transform={`translate(${pos.x}, ${pos.y})`}
                style={{ cursor: "pointer" }}
                onClick={() => onOpenPage(node.path)}
              >
                <circle r={22} fill="var(--accent)" opacity={0.16} />
                <circle r={10} fill="var(--accent)" />
                <text
                  x={0}
                  y={34}
                  textAnchor="middle"
                  fill="var(--text-primary)"
                  fontSize={12}
                  fontWeight={700}
                >
                  {node.label.length > 18 ? `${node.label.slice(0, 18)}...` : node.label}
                </text>
                <text x={0} y={50} textAnchor="middle" fill="var(--text-muted)" fontSize={10}>
                  {node.pageType}
                </text>
              </g>
            );
          })}
        </svg>
      </div>
    </div>
  );
}
