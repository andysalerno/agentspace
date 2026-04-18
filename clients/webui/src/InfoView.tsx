import { useEffect, useState } from "react";
import { api } from "./api";
import type { ServiceInfoSection, SystemInfo } from "./types";

function Section({ title, section }: { title: string; section: ServiceInfoSection | undefined }) {
  if (!section) {
    return (
      <section className="info-section card">
        <div className="card-body">
          <h3>{title}</h3>
          <p className="muted">No data.</p>
        </div>
      </section>
    );
  }

  const env = section.env ?? {};
  const entries = Object.entries(env).sort(([a], [b]) => a.localeCompare(b));

  return (
    <section className="info-section card">
      <div className="card-body">
        <h3>{title}</h3>
        {section.env_prefix && (
          <p className="muted">
            Env vars matching <code>{section.env_prefix}*</code>
          </p>
        )}
        {section.error && <p className="info-error">{section.error}</p>}
        {entries.length === 0 && !section.error ? (
          <p className="muted">No matching environment variables.</p>
        ) : (
          <table className="info-table">
            <thead>
              <tr>
                <th>Variable</th>
                <th>Value</th>
              </tr>
            </thead>
            <tbody>
              {entries.map(([name, value]) => (
                <tr key={name}>
                  <td><code>{name}</code></td>
                  <td><code>{value}</code></td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </section>
  );
}

export default function InfoView() {
  const [info, setInfo] = useState<SystemInfo | null>(null);
  const [webuiInfo, setWebuiInfo] = useState<ServiceInfoSection | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  async function load() {
    setLoading(true);
    setError(null);
    const [systemResult, webuiResult] = await Promise.allSettled([
      api.getInfo(),
      api.getWebuiInfo(),
    ]);
    if (systemResult.status === "fulfilled") {
      setInfo(systemResult.value);
    } else {
      setError(systemResult.reason instanceof Error ? systemResult.reason.message : String(systemResult.reason));
    }
    if (webuiResult.status === "fulfilled") {
      setWebuiInfo(webuiResult.value);
    } else {
      setWebuiInfo({
        service: "webui",
        env_prefix: "WEBUI_CLIENT",
        error: webuiResult.reason instanceof Error ? webuiResult.reason.message : String(webuiResult.reason),
      });
    }
    setLoading(false);
  }

  useEffect(() => {
    void load();
  }, []);

  return (
    <div className="view-content">
      <div className="view-header">
        <h2>System Info</h2>
        <button
          className="secondary-button small"
          onClick={() => void load()}
          type="button"
          disabled={loading}
        >
          {loading ? "Refreshing…" : "Refresh"}
        </button>
      </div>
      {error && <div className="error-banner"><span>{error}</span></div>}
      <div className="info-sections">
        <Section title="agent_host" section={info?.agent_host} />
        <Section title="client_service" section={info?.client_service} />
        <Section title="webui" section={webuiInfo ?? undefined} />
      </div>
    </div>
  );
}
