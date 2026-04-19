import { useQueryClient } from "@tanstack/react-query";
import type { ServiceInfoSection } from "./types";
import { queryKeys, useSystemInfo, useWebuiInfo } from "./queries";

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
  const systemInfoQuery = useSystemInfo();
  const webuiInfoQuery = useWebuiInfo();
  const queryClient = useQueryClient();

  const loading = systemInfoQuery.isFetching || webuiInfoQuery.isFetching;
  const error = systemInfoQuery.error;
  const webuiInfo: ServiceInfoSection | undefined = webuiInfoQuery.data ?? (
    webuiInfoQuery.error
      ? {
          service: "webui",
          env_prefix: "WEBUI_CLIENT",
          error:
            webuiInfoQuery.error instanceof Error
              ? webuiInfoQuery.error.message
              : String(webuiInfoQuery.error),
        }
      : undefined
  );

  function refresh() {
    void queryClient.invalidateQueries({ queryKey: queryKeys.systemInfo });
    void queryClient.invalidateQueries({ queryKey: queryKeys.webuiInfo });
  }

  return (
    <div className="view-content">
      <div className="view-header">
        <h2>System Info</h2>
        <button
          className="secondary-button small"
          onClick={refresh}
          type="button"
          disabled={loading}
        >
          {loading ? "Refreshing…" : "Refresh"}
        </button>
      </div>
      {error && (
        <div className="error-banner">
          <span>{error instanceof Error ? error.message : String(error)}</span>
        </div>
      )}
      <div className="info-sections">
        <Section title="agent_host" section={systemInfoQuery.data?.agent_host} />
        <Section title="client_service" section={systemInfoQuery.data?.client_service} />
        <Section title="webui" section={webuiInfo} />
      </div>
    </div>
  );
}
