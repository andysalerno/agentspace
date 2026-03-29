import type { KernelSummary } from "./types";

type KernelsViewProps = {
  kernels: KernelSummary[];
};

export default function KernelsView({ kernels }: KernelsViewProps) {
  return (
    <div className="view-content">
      <div className="view-header">
        <h2>Kernels</h2>
        <span className="muted">{kernels.length} active</span>
      </div>

      {kernels.length > 0 ? (
        <div className="card-grid">
          {kernels.map((kernel) => (
            <div className="card" key={kernel.session_id}>
              <div className="card-body">
                <h3>{kernel.harness}</h3>
                <div className="mono muted">{kernel.session_id.slice(0, 8)}…</div>
                <div className="detail-grid">
                  <span className="detail-label">Status</span>
                  <span className={`status-badge ${kernel.status}`}>{kernel.status}</span>
                  <span className="detail-label">Turns</span>
                  <span>{kernel.turns}</span>
                  <span className="detail-label">CWD</span>
                  <span className="mono">{kernel.cwd ?? "—"}</span>
                </div>
                {kernel.client_session_ids.length > 0 && (
                  <div className="tag-row">
                    {kernel.client_session_ids.map((id) => (
                      <span className="tag" key={id}>
                        {id.slice(0, 8)}…
                      </span>
                    ))}
                  </div>
                )}
              </div>
            </div>
          ))}
        </div>
      ) : (
        <div className="empty-state">No active kernels.</div>
      )}
    </div>
  );
}
