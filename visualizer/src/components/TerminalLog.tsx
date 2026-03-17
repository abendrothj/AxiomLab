import { useEffect, useRef, useState } from "react";

interface Props {
  tokens: string;
}

// 50ms buffer flush — prevents ~200 re-renders/sec from char streaming
const FLUSH_INTERVAL_MS = 50;

export default function TerminalLog({ tokens }: Props) {
  const [displayed, setDisplayed] = useState("");
  const pendingRef = useRef("");
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);

  // Diff: accumulate only the new suffix
  const prevTokensRef = useRef("");
  useEffect(() => {
    const newPart = tokens.slice(prevTokensRef.current.length);
    prevTokensRef.current = tokens;
    if (!newPart) return;

    pendingRef.current += newPart;

    if (!timerRef.current) {
      timerRef.current = setInterval(() => {
        if (pendingRef.current) {
          const chunk = pendingRef.current;
          pendingRef.current = "";
          setDisplayed((prev) => prev + chunk);
        }
      }, FLUSH_INTERVAL_MS);
    }
  }, [tokens]);

  // Stop flush timer when fully drained
  useEffect(() => {
    return () => {
      if (timerRef.current) clearInterval(timerRef.current);
    };
  }, []);

  // Auto-scroll to bottom whenever displayed text grows
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [displayed]);

  const isEmpty = displayed.length === 0;

  return (
    <div
      style={{
        width: "100%",
        height: "100%",
        background: "#0a0d12",
        display: "flex",
        flexDirection: "column",
        overflow: "hidden",
        position: "relative",
      }}
    >
      {/* Panel label */}
      <div
        style={{
          padding: "4px 10px",
          fontSize: 9,
          color: "#1a3a4a",
          letterSpacing: "0.14em",
          borderBottom: "1px solid #0d1a25",
          flexShrink: 0,
        }}
      >
        LLM OUTPUT
      </div>

      {/* Scrollable text area */}
      <div
        ref={scrollRef}
        style={{
          flex: 1,
          overflowY: "auto",
          padding: "8px 12px 8px 10px",
          fontFamily: '"JetBrains Mono", "Fira Code", monospace',
          fontSize: 11,
          lineHeight: 1.65,
          color: "#b0d0b0",
          whiteSpace: "pre-wrap",
          wordBreak: "break-word",
        }}
      >
        {isEmpty ? (
          <span style={{ color: "#1a3a2a", fontSize: 10 }}>awaiting llm stream...</span>
        ) : (
          <>
            {displayed}
            {/* Blinking cursor */}
            <span
              style={{
                display: "inline-block",
                width: 7,
                height: 13,
                background: "#00ff9d",
                marginLeft: 2,
                verticalAlign: "text-bottom",
                animation: "blink 1s step-end infinite",
              }}
            />
          </>
        )}
      </div>
    </div>
  );
}
