import { useQueryClient } from "@tanstack/react-query";
import { ArrowClockwise20Regular } from "@fluentui/react-icons";
import type { ServiceInfoSection } from "./types";
import { queryKeys, useSystemInfo, useWebuiInfo } from "./queries";
import { Button, MessageBar, MessageBarBody } from "./fluent";
import { ViewHeader } from "./ui";

function Section(
    { title, section }: { title: string; section: ServiceInfoSection | undefined },
) {
    const env = section?.env ?? {};
    const entries = Object.entries(env).sort(([a], [b]) => a.localeCompare(b));

    return (
        <section className="panel">
            <div className="panel-header">
                <div className="info-section-title">
                    <h3>{title}</h3>
                    {section?.env_prefix && (
                        <span className="muted-sm">{section.env_prefix}*</span>
                    )}
                </div>
            </div>
            <div className="panel-body">
                {section === undefined && <p className="muted">No data reported.</p>}
                {section?.error && (
                    <MessageBar intent="error">
                        <MessageBarBody>{section.error}</MessageBarBody>
                    </MessageBar>
                )}
                {section !== undefined && entries.length === 0 && !section.error && (
                    <p className="muted">No matching environment variables.</p>
                )}
                {entries.length > 0 && (
                    <div className="table-scroll">
                        <table className="data-table info-table">
                            <thead>
                                <tr>
                                    <th>Variable</th>
                                    <th>Value</th>
                                </tr>
                            </thead>
                            <tbody>
                                {entries.map(([name, value]) => (
                                    <tr key={name}>
                                        <td>{name}</td>
                                        <td>{value}</td>
                                    </tr>
                                ))}
                            </tbody>
                        </table>
                    </div>
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
                error: webuiInfoQuery.error instanceof Error
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
            <ViewHeader
                actions={
                    <Button
                        disabled={loading}
                        icon={<ArrowClockwise20Regular />}
                        onClick={refresh}
                        type="button"
                    >
                        {loading ? "Refreshing…" : "Refresh"}
                    </Button>
                }
                description="Runtime environment reported by each AgentSpace service."
                title="System info"
            />
            <div className="view-body">
                {error && (
                    <MessageBar intent="error">
                        <MessageBarBody>
                            {error instanceof Error ? error.message : String(error)}
                        </MessageBarBody>
                    </MessageBar>
                )}
                <div className="info-sections">
                    <Section section={systemInfoQuery.data?.agent_host} title="agent_host" />
                    <Section
                        section={systemInfoQuery.data?.client_service}
                        title="client_service"
                    />
                    <Section section={webuiInfo} title="webui" />
                </div>
            </div>
        </div>
    );
}
