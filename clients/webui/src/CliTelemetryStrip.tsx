import type { ReactNode } from "react";
import { useId, useState } from "react";
import { Dismiss20Regular } from "@fluentui/react-icons";
import { Button, Tooltip } from "./fluent";
import {
    CACHE_REUSE_TOOLTIP,
    CONTEXT_OCCUPANCY_TOOLTIP,
    coverageStateLabel,
    formatCacheSummary,
    formatCompactCount,
    formatContextSummary,
    formatCoverage,
    formatDuration,
    formatExactCount,
    formatLatestCallSummary,
    formatOpaqueCost,
    formatPercent,
    formatSessionTotalSummary,
    formatSubagentSummary,
    formatUsageValue,
    formatWarnings,
    humanizeSnakeCase,
    shouldShowObservedCounts,
    telemetryContentModeLabel,
    telemetryDisplayReason,
    telemetryDisplayState,
    telemetryStateTone,
    telemetryStatusAge,
    usageReported,
} from "./telemetry";
import type { TelemetrySnapshot, UsageBreakdown } from "./types";
import { StatusBadge } from "./ui";

type CliTelemetryStripProps = {
    telemetry: TelemetrySnapshot | undefined;
    telemetryError: unknown;
    telemetryPending: boolean;
    dataUpdatedAt: number;
};

function valueOrPlaceholder(value: string | null | undefined): string {
    if (value === null || value === undefined || value.trim() === "") {
        return "Not reported";
    }
    return value;
}

function ageLabel(age: string | null, state: string): string | null {
    if (age === null) {
        return null;
    }
    return state === "stale" ? `${age} old` : `${age} ago`;
}

function cacheSignalLabel(snapshot: TelemetrySnapshot | undefined): string {
    if (snapshot?.cache_signal === null || snapshot?.cache_signal === undefined) {
        return "Not reported";
    }
    const { state, confidence, reason } = snapshot.cache_signal;
    const parts = [humanizeSnakeCase(state)];
    if (confidence !== null) {
        parts.push(humanizeSnakeCase(confidence));
    }
    if (reason !== null) {
        parts.push(humanizeSnakeCase(reason));
    }
    return parts.join(" · ");
}

function contextDetailValue(snapshot: TelemetrySnapshot | undefined): string {
    if (snapshot?.context === null || snapshot?.context === undefined) {
        return snapshot?.state === "unavailable" ? "Not available" : "Not reported";
    }
    const { tokens, limit } = snapshot.context;
    if (tokens !== null && limit !== null) {
        return `${formatExactCount(tokens)} / ${formatExactCount(limit)}`;
    }
    if (tokens !== null) {
        return `${formatExactCount(tokens)} / limit unknown`;
    }
    if (limit !== null) {
        return `Usage unknown / ${formatExactCount(limit)}`;
    }
    return "Not reported";
}

function sessionUsageUnavailable(snapshot: TelemetrySnapshot | undefined): boolean {
    return snapshot !== undefined
        && snapshot.state === "unavailable"
        && !usageReported(snapshot.session);
}

function DetailHint({ label, content }: { label: string; content: string }) {
    return (
        <Tooltip content={content} relationship="description">
            <span
                aria-label={`${label} help`}
                className="cli-telemetry-help"
                role="img"
                tabIndex={0}
            >
                ⓘ
            </span>
        </Tooltip>
    );
}

function SummaryItem(
    { label, value, tooltip }: { label: string; value: string; tooltip?: string },
) {
    return (
        <div className="cli-telemetry-item">
            <span className="cli-telemetry-item-label">
                {label}
                {tooltip !== undefined && <DetailHint content={tooltip} label={label} />}
            </span>
            <span className="cli-telemetry-item-value">{value}</span>
        </div>
    );
}

function DetailsSection(
    { title, children }: { title: string; children: ReactNode },
) {
    return (
        <section className="cli-telemetry-section">
            <h3>{title}</h3>
            {children}
        </section>
    );
}

function DetailsGrid({ children }: { children: ReactNode }) {
    return <dl className="cli-telemetry-grid">{children}</dl>;
}

function DetailRow(
    { label, value, tooltip }: { label: string; value: ReactNode; tooltip?: string },
) {
    return (
        <>
            <dt>
                <span className="cli-telemetry-row-label">
                    {label}
                    {tooltip !== undefined && <DetailHint content={tooltip} label={label} />}
                </span>
            </dt>
            <dd>{value}</dd>
        </>
    );
}

function UsageRows({ usage }: { usage: UsageBreakdown }) {
    return (
        <DetailsGrid>
            <DetailRow label="Effective input" value={formatUsageValue(usage.effective_input_tokens)} />
            <DetailRow label="Output" value={formatUsageValue(usage.output_tokens)} />
            <DetailRow label="Total" value={formatUsageValue(usage.total_tokens)} />
            <DetailRow label="Raw input" value={formatUsageValue(usage.raw_input_tokens)} />
            <DetailRow
                label="Cache reuse"
                tooltip={CACHE_REUSE_TOOLTIP}
                value={formatPercent(usage.cache_reuse_percent)}
            />
            <DetailRow label="Cache read" value={formatUsageValue(usage.cache_read_input_tokens)} />
            <DetailRow label="Cache write" value={formatUsageValue(usage.cache_write_input_tokens)} />
            <DetailRow label="Other input" value={formatUsageValue(usage.other_input_tokens)} />
            <DetailRow label="Fresh input" value={formatUsageValue(usage.fresh_input_tokens)} />
            <DetailRow label="Reasoning output" value={formatUsageValue(usage.reasoning_output_tokens)} />
            <DetailRow label="Nano-AIU" value={formatUsageValue(usage.nano_aiu)} />
            <DetailRow label="Opaque cost" value={formatOpaqueCost(usage.opaque_cost)} />
        </DetailsGrid>
    );
}

export default function CliTelemetryStrip({
    telemetry,
    telemetryError,
    telemetryPending,
    dataUpdatedAt,
}: CliTelemetryStripProps) {
    const [detailsOpen, setDetailsOpen] = useState(false);
    const detailsId = useId();
    const titleId = useId();

    const state = telemetryDisplayState(telemetry, telemetryError, telemetryPending);
    const reason = telemetryDisplayReason(telemetry, telemetryError, telemetryPending);
    const age = ageLabel(telemetryStatusAge(telemetry, dataUpdatedAt), state);
    const totalSummary = telemetry === undefined
        ? (state === "starting" ? "Waiting for totals" : "Not available")
        : formatSessionTotalSummary(telemetry.session, state);
    const latestSummary = telemetry === undefined
        ? (state === "starting" ? "Waiting for first call" : "Not available")
        : formatLatestCallSummary(telemetry.latest_call, state);
    const cacheSummary = formatCacheSummary(telemetry, state);
    const contextSummary = telemetry === undefined
        ? (state === "starting" ? "Waiting for context" : "Not available")
        : formatContextSummary(telemetry.context, state);
    const subagentSummary = formatSubagentSummary(telemetry, state);
    const observedCounts = telemetry !== undefined && shouldShowObservedCounts(telemetry);

    return (
        <div aria-label="CLI telemetry summary" className="cli-telemetry" role="group">
            <div className="cli-telemetry-strip">
                <div className="cli-telemetry-status">
                    <StatusBadge label={state} tone={telemetryStateTone(state)} />
                    {age !== null && <span className="cli-telemetry-age">{age}</span>}
                    {telemetry !== undefined && telemetryError !== null && telemetryError !== undefined && (
                        <span className="cli-telemetry-note">retained browser data</span>
                    )}
                </div>

                <div
                    aria-hidden="true"
                    className="cli-telemetry-summary cli-telemetry-summary-wide"
                >
                    <SummaryItem label="Session" value={totalSummary} />
                    <SummaryItem label="Latest" value={latestSummary} />
                    <SummaryItem label="Cache" tooltip={CACHE_REUSE_TOOLTIP} value={cacheSummary} />
                    <SummaryItem
                        label="Context"
                        tooltip={CONTEXT_OCCUPANCY_TOOLTIP}
                        value={contextSummary}
                    />
                    <SummaryItem label="Agents" value={subagentSummary} />
                </div>

                <div className="cli-telemetry-summary cli-telemetry-summary-narrow">
                    <span>{totalSummary}</span>
                    <span aria-hidden="true">|</span>
                    <span>{cacheSummary}</span>
                    <span aria-hidden="true">|</span>
                    <span>{subagentSummary}</span>
                </div>

                <Button
                    appearance="subtle"
                    aria-controls={detailsId}
                    aria-expanded={detailsOpen}
                    className="cli-telemetry-details-button"
                    onClick={() => setDetailsOpen((open) => !open)}
                    size="small"
                >
                    Usage details
                </Button>
            </div>

            {reason !== null && (
                <div className="cli-telemetry-reason" role={state === "degraded" ? "alert" : undefined}>
                    {reason}
                </div>
            )}

            {detailsOpen && (
                <section
                    aria-labelledby={titleId}
                    aria-modal="false"
                    className="cli-telemetry-details-surface"
                    id={detailsId}
                    role="dialog"
                    tabIndex={-1}
                >
                    <div className="cli-telemetry-details-header">
                        <div>
                            <h3 id={titleId}>CLI telemetry details</h3>
                            <p>
                                Latest call is shown here. Last interaction remains hidden until
                                grouping is verified.
                            </p>
                        </div>
                        <Button
                            appearance="subtle"
                            aria-label="Close telemetry details"
                            icon={<Dismiss20Regular />}
                            onClick={() => setDetailsOpen(false)}
                            size="small"
                        />
                    </div>

                    <div className="cli-telemetry-sections">
                        <DetailsSection title="Session usage">
                            {telemetry === undefined
                                ? <p className="muted">Waiting for telemetry.</p>
                                : sessionUsageUnavailable(telemetry)
                                ? <p className="muted">Usage is unavailable for this session.</p>
                                : <UsageRows usage={telemetry.session} />}
                        </DetailsSection>

                        <DetailsSection title="Latest model call">
                            {telemetry?.latest_call === null || telemetry?.latest_call === undefined
                                ? <p className="muted">No completed model call has been reported yet.</p>
                                : (
                                    <>
                                        <DetailsGrid>
                                            <DetailRow
                                                label="Model"
                                                value={valueOrPlaceholder(telemetry.latest_call.model)}
                                            />
                                            <DetailRow
                                                label="Requested model"
                                                value={valueOrPlaceholder(
                                                    telemetry.latest_call.requested_model,
                                                )}
                                            />
                                            <DetailRow
                                                label="Provider"
                                                value={valueOrPlaceholder(
                                                    telemetry.latest_call.provider,
                                                )}
                                            />
                                            <DetailRow
                                                label="Agent ID"
                                                value={valueOrPlaceholder(
                                                    telemetry.latest_call.agent_id,
                                                )}
                                            />
                                            <DetailRow
                                                label="Agent name"
                                                value={valueOrPlaceholder(
                                                    telemetry.latest_call.agent_name,
                                                )}
                                            />
                                            <DetailRow
                                                label="Subagent"
                                                value={telemetry.latest_call.is_subagent ? "Yes" : "No"}
                                            />
                                            <DetailRow
                                                label="Started"
                                                value={valueOrPlaceholder(
                                                    telemetry.latest_call.started_at,
                                                )}
                                            />
                                            <DetailRow
                                                label="Ended"
                                                value={valueOrPlaceholder(
                                                    telemetry.latest_call.ended_at,
                                                )}
                                            />
                                            <DetailRow
                                                label="Duration"
                                                value={formatDuration(
                                                    telemetry.latest_call.duration_ms,
                                                )}
                                            />
                                            <DetailRow
                                                label="Cache reporting"
                                                value={humanizeSnakeCase(
                                                    telemetry.latest_call.cache_reporting,
                                                )}
                                            />
                                            <DetailRow
                                                label="Accounting"
                                                value={humanizeSnakeCase(
                                                    telemetry.latest_call
                                                        .token_accounting_convention,
                                                )}
                                            />
                                        </DetailsGrid>
                                        <UsageRows usage={telemetry.latest_call.usage} />
                                    </>
                                )}
                        </DetailsSection>

                        <DetailsSection title="Coverage & counts">
                            {telemetry === undefined
                                ? <p className="muted">Waiting for telemetry.</p>
                                : !observedCounts
                                ? <p className="muted">Counts are unavailable while telemetry is unavailable.</p>
                                : (
                                    <DetailsGrid>
                                        <DetailRow
                                            label="Coverage state"
                                            value={coverageStateLabel(telemetry.reporting)}
                                        />
                                        <DetailRow
                                            label="Cache reported calls"
                                            value={formatCoverage(
                                                telemetry.reporting.cache_reported_calls,
                                                telemetry.reporting.model_calls,
                                            )}
                                        />
                                        <DetailRow
                                            label="Accounting resolved"
                                            value={formatCoverage(
                                                telemetry.reporting.convention_resolved_calls,
                                                telemetry.reporting.model_calls,
                                            )}
                                        />
                                        <DetailRow
                                            label="Effective input covered"
                                            value={formatCoverage(
                                                telemetry.reporting.effective_input_covered_calls,
                                                telemetry.reporting.model_calls,
                                            )}
                                        />
                                        <DetailRow
                                            label="Context reported"
                                            value={telemetry.reporting.context_reported
                                                ? "Yes"
                                                : "No"}
                                        />
                                        <DetailRow
                                            label="Interactions"
                                            value={formatCompactCount(
                                                telemetry.counts.interactions,
                                            )}
                                        />
                                        <DetailRow
                                            label="Model calls"
                                            value={formatCompactCount(
                                                telemetry.counts.model_calls,
                                            )}
                                        />
                                        <DetailRow
                                            label="Tool calls"
                                            value={formatCompactCount(
                                                telemetry.counts.tool_calls,
                                            )}
                                        />
                                        <DetailRow
                                            label="Subagent invocations"
                                            value={formatCompactCount(
                                                telemetry.counts.subagent_invocations,
                                            )}
                                        />
                                        <DetailRow
                                            label="Subagent model calls"
                                            value={formatCompactCount(
                                                telemetry.counts.subagent_model_calls,
                                            )}
                                        />
                                        <DetailRow
                                            label="Errors"
                                            value={formatCompactCount(telemetry.counts.errors)}
                                        />
                                    </DetailsGrid>
                                )}
                        </DetailsSection>

                        <DetailsSection title="Subagent usage">
                            {telemetry === undefined
                                ? <p className="muted">Waiting for telemetry.</p>
                                : !observedCounts
                                ? <p className="muted">Subagent usage is unavailable while telemetry is unavailable.</p>
                                : (
                                    <DetailsGrid>
                                        <DetailRow
                                            label="Invocations"
                                            value={formatCompactCount(
                                                telemetry.subagents.invocations,
                                            )}
                                        />
                                        <DetailRow
                                            label="Model calls"
                                            value={formatCompactCount(
                                                telemetry.subagents.model_calls,
                                            )}
                                        />
                                        <DetailRow
                                            label="Effective input"
                                            value={formatUsageValue(
                                                telemetry.subagents.effective_input_tokens,
                                            )}
                                        />
                                        <DetailRow
                                            label="Output"
                                            value={formatUsageValue(
                                                telemetry.subagents.output_tokens,
                                            )}
                                        />
                                        <DetailRow
                                            label="Cache read"
                                            value={formatUsageValue(
                                                telemetry.subagents.cache_read_input_tokens,
                                            )}
                                        />
                                        <DetailRow
                                            label="Cache write"
                                            value={formatUsageValue(
                                                telemetry.subagents.cache_write_input_tokens,
                                            )}
                                        />
                                        <DetailRow
                                            label="Total duration"
                                            value={formatDuration(
                                                telemetry.subagents.duration_ms,
                                            )}
                                        />
                                    </DetailsGrid>
                                )}
                        </DetailsSection>

                        <DetailsSection title="Context & freshness">
                            <DetailsGrid>
                                <DetailRow
                                    label="Context occupancy"
                                    tooltip={CONTEXT_OCCUPANCY_TOOLTIP}
                                    value={contextDetailValue(telemetry)}
                                />
                                <DetailRow
                                    label="Messages in context"
                                    value={telemetry?.context?.message_count === null
                                        || telemetry?.context?.message_count === undefined
                                        ? (telemetry?.state === "unavailable"
                                            ? "Not available"
                                            : "Not reported")
                                        : formatExactCount(telemetry.context.message_count)}
                                />
                                <DetailRow
                                    label="Context observed at"
                                    value={valueOrPlaceholder(telemetry?.context?.observed_at)}
                                />
                                <DetailRow
                                    label="Snapshot observed at"
                                    value={valueOrPlaceholder(telemetry?.observed_at)}
                                />
                                <DetailRow
                                    label="Snapshot received at"
                                    value={valueOrPlaceholder(telemetry?.received_at)}
                                />
                                <DetailRow
                                    label="Telemetry age"
                                    value={age ?? "Unknown"}
                                />
                                <DetailRow
                                    label="System prompt tokens"
                                    value="Not reported by Copilot"
                                />
                                <DetailRow
                                    label="Cache signal"
                                    value={cacheSignalLabel(telemetry)}
                                />
                            </DetailsGrid>
                        </DetailsSection>

                        <DetailsSection title="Health & policy">
                            <DetailsGrid>
                                <DetailRow label="Telemetry state" value={state} />
                                <DetailRow
                                    label="Content mode"
                                    value={telemetry === undefined
                                        ? "Not reported"
                                        : telemetryContentModeLabel(
                                            telemetry.content_mode,
                                        )}
                                />
                                <DetailRow
                                    label="Source version"
                                    value={valueOrPlaceholder(telemetry?.source_version)}
                                />
                                <DetailRow
                                    label="Warnings"
                                    value={telemetry === undefined
                                        ? "Not reported"
                                        : formatWarnings(telemetry)}
                                />
                                <DetailRow
                                    label="Reason"
                                    value={reason ?? "None"}
                                />
                            </DetailsGrid>
                        </DetailsSection>
                    </div>
                </section>
            )}
        </div>
    );
}
