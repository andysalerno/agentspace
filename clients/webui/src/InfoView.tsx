import { useQueryClient } from "@tanstack/react-query";
import type { ServiceInfoSection } from "./types";
import { queryKeys, useSystemInfo, useWebuiInfo } from "./queries";
import {
  Button,
  Table,
  TableBody,
  TableCell,
  TableHeader,
  TableHeaderCell,
  TableRow,
} from "./fluent";

function Section({ title, section }: { title: string; section: ServiceInfoSection | undefined }) {
  if (!section) {
    return (
      <div className="info-section card management-card">
        <div className="card-body">
          <h3>{title}</h3>
          <p className="muted">No data.</p>
        </div>
      </div>
    );
  }

  const env = section.env ?? {};
  const entries = Object.entries(env).sort(([a], [b]) => a.localeCompare(b));

  return (
    <div className="info-section card management-card">
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
          <Table className="info-table management-table">
            <TableHeader>
              <TableRow>
                <TableHeaderCell>Variable</TableHeaderCell>
                <TableHeaderCell>Value</TableHeaderCell>
              </TableRow>
            </TableHeader>
            <TableBody>
              {entries.map(([name, value]) => (
                <TableRow key={name}>
                  <TableCell><code>{name}</code></TableCell>
                  <TableCell><code>{value}</code></TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </div>
    </div>
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
    <div className="view-content management-view info-management-view">
      <div className="view-header">
        <div>
          <h2>System Info</h2>
          <span className="muted">Runtime environment across AgentSpace services</span>
        </div>
        <div className="view-header-actions">
          <Button
            className="secondary-button small"
            onClick={refresh}
            type="button"
            disabled={loading}
          >
            {loading ? "Refreshing…" : "Refresh"}
          </Button>
        </div>
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
