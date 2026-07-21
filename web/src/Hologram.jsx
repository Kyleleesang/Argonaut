import { Html } from "@react-three/drei";

/*
 * A holographic readout pinned beside a celestial body: a projected panel with
 * a leader line back to the body it annotates.
 *
 * Only rendered for the body the user has actually picked (via a tab or by
 * clicking the body itself) -- annotating everything at once turns the scene
 * into a wall of text. The collapsed/expanded CSS states are still wired up
 * via `focused` in case compact always-on tags are wanted later.
 *
 * Rendered as DOM (drei's Html) rather than in-scene text so the type stays
 * crisp at any distance and can use real CSS (blur, scanlines, glow). No
 * distanceFactor for the same reason -- a far body's label should still read.
 */
export function Hologram({ body, offset, focused }) {
  const { name, role, cls, summary, facts, accent } = body;
  return (
    <Html position={offset} zIndexRange={[8, 0]} style={{ pointerEvents: "none" }}>
      <div className={`holo${focused ? " on" : ""}`} style={{ "--holo": accent || "#8fb8ff" }}>
        <span className="holo-tether" />
        <div className="holo-panel">
          <div className="holo-head">
            <span className="holo-dot" />
            <span className="holo-name">{name}</span>
            <span className="holo-role">{role}</span>
          </div>
          <div className="holo-body">
            <p className="holo-cls">{cls}</p>
            <p className="holo-sum">{summary}</p>
            {facts?.length > 0 && (
              <ul className="holo-facts">
                {facts.map((f) => (
                  <li key={f}>{f}</li>
                ))}
              </ul>
            )}
          </div>
        </div>
      </div>
    </Html>
  );
}
