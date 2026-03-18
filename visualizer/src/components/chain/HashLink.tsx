/// Visual connector between two audit event blocks in the chain view.
///
/// Renders a thin vertical line with the truncated hash displayed on hover.
/// Colour: green when the chain link is intact, red when broken.

interface HashLinkProps {
  prevHash: string;
  entryHash: string;
  broken?: boolean;
}

export default function HashLink({ prevHash, entryHash, broken = false }: HashLinkProps) {
  const color = broken ? "#ff4444" : "#1a3a50";
  const glowColor = broken ? "#ff444460" : "#00d4ff20";

  return (
    <div
      title={`prev: ${prevHash}\n→ ${entryHash}`}
      style={{
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        gap: 0,
        flexShrink: 0,
      }}
    >
      {/* Vertical connector line */}
      <div style={{
        width: 1,
        height: 20,
        background: `linear-gradient(to bottom, ${color}, ${color}88)`,
        boxShadow: broken ? `0 0 4px ${glowColor}` : "none",
        position: "relative",
      }} />

      {/* Hash pill */}
      <div style={{
        fontSize: 8,
        color: broken ? "#ff6666" : "#1a4a5a",
        letterSpacing: "0.08em",
        fontFamily: '"JetBrains Mono", "Fira Code", monospace',
        padding: "1px 6px",
        background: broken ? "#2a0a0a" : "#0a0e18",
        border: `1px solid ${broken ? "#4a1a1a" : "#0e1824"}`,
        borderRadius: 2,
        cursor: "default",
        whiteSpace: "nowrap",
      }}>
        {prevHash ? prevHash.slice(0, 8) + "…" : "genesis"}
      </div>

      {/* Vertical connector line (below pill) */}
      <div style={{
        width: 1,
        height: 20,
        background: `linear-gradient(to bottom, ${color}88, ${color})`,
        boxShadow: broken ? `0 0 4px ${glowColor}` : "none",
      }} />
    </div>
  );
}
