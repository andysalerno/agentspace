import { WebUIElement, observable } from "@microsoft/webui-framework";

import { api } from "../api";
import {
  createEmptyAppState,
  createStatusRows,
  createSummaryCards,
  createSystemSections,
  DEFAULT_AGENT_SYSTEM_PROMPT,
  DEFAULT_HARNESS,
  emptyAgentForm,
  emptyConnectionForm,
  emptyGatewayForm,
  emptyGitRequestDetail,
  emptySkillForm,
  emptyWorkspaceForm,
  formatDate,
  normalizeGitRequestDetail,
  normalizeGitRequests,
  normalizeKernels,
  normalizeSessions,
  VIEW_META,
} from "../state";
import type {
  AcpSessionUpdate,
  Agent,
  AgentFormState,
  ChatMessage,
  Connection,
  ConnectionFormState,
  Gateway,
  GatewayFormState,
  GatewayType,
  GitAgentConfig,
  GitAgentConfigFormState,
  GitAgentRequestsResponse,
  GitAgentStatus,
  Harness,
  InfoSection,
  KernelEvent,
  MessageStreamFinalChunk,
  ServiceInfoSection,
  Skill,
  SkillVersion,
  SystemInfo,
  UiChatMessage,
  UiGitAgentRequest,
  UiGitAgentRequestDetail,
  UiKernelSummary,
  UiSessionSummary,
  UiToolCall,
  ViewId,
  Workspace,
  WorkspaceMount,
} from "../types";

type ValueControl = HTMLElement & {
  value?: string;
  checked?: boolean;
};

type WorkspaceMountInput = Pick<WorkspaceMount, "workspace_id" | "mode">;

const emptyState = createEmptyAppState();

class AgentspaceApp extends WebUIElement {
  @observable title = emptyState.title;
  @observable textdirection = emptyState.textdirection;
  @observable theme = emptyState.theme;
  @observable darkMode = emptyState.darkMode;
  @observable sidebarCollapsed = emptyState.sidebarCollapsed;
  @observable generatedAtLabel = emptyState.generatedAtLabel;
  @observable currentView = emptyState.currentView;
  @observable currentViewTitle = emptyState.currentViewTitle;
  @observable currentViewDescription = emptyState.currentViewDescription;
  @observable navItems = emptyState.navItems;
  @observable summaryCards = emptyState.summaryCards;
  @observable harnesses: Harness[] = [];
  @observable agents: Agent[] = [];
  @observable workspaces: Workspace[] = [];
  @observable sessions: UiSessionSummary[] = [];
  @observable kernels: UiKernelSummary[] = [];
  @observable skills: Skill[] = [];
  @observable skillVersions: SkillVersion[] = [];
  @observable connections: Connection[] = [];
  @observable gateways: Gateway[] = [];
  @observable gatewayTypes: GatewayType[] = [];
  @observable gitAgentStatusRows = emptyState.gitAgentStatusRows;
  @observable gitAgentRequests: UiGitAgentRequest[] = [];
  @observable selectedGitRequest: UiGitAgentRequestDetail = emptyGitRequestDetail();
  @observable systemSections: InfoSection[] = emptyState.systemSections;
  @observable error = "";
  @observable isRefreshing = false;
  @observable selectedSessionId = "";
  @observable selectedSessionTitle = emptyState.selectedSessionTitle;
  @observable chatMessages: UiChatMessage[] = [];
  @observable isStreaming = false;
  @observable showAgentForm = false;
  @observable isEditingAgent = false;
  @observable agentForm = emptyAgentForm();
  @observable workspaceForm = emptyWorkspaceForm();
  @observable showWorkspaceForm = false;
  @observable showLogs = false;
  @observable logsTitle = "";
  @observable logSource: "harness" | "container" = "harness";
  @observable logLines: string[] = [];
  @observable showSkillForm = false;
  @observable selectedSkillId = "";
  @observable skillForm = emptySkillForm();
  @observable showConnectionForm = false;
  @observable isEditingConnection = false;
  @observable connectionForm = emptyConnectionForm();
  @observable selectedConnectionModelsText = "";
  @observable showGatewayForm = false;
  @observable isEditingGateway = false;
  @observable gatewayForm = emptyGatewayForm();
  @observable showGatewayLogs = false;
  @observable gatewayLogsTitle = "";
  @observable gatewayLogLines: string[] = [];
  @observable selectedKernelConfigHarness = "";
  @observable kernelConfigEnv = "";
  @observable gitAgentConfig: GitAgentConfigFormState = emptyState.gitAgentConfig;

  newSessionAgentSelect!: ValueControl;
  composerInput!: ValueControl;
  agentIdInput!: ValueControl;
  agentNameInput!: ValueControl;
  agentHarnessSelect!: ValueControl;
  agentConnectionSelect!: ValueControl;
  agentPromptInput!: ValueControl;
  agentSkillsInput!: ValueControl;
  agentEnvInput!: ValueControl;
  agentMountsInput!: ValueControl;
  workspaceIdInput!: ValueControl;
  workspaceNameInput!: ValueControl;
  skillIdInput!: ValueControl;
  skillFilesInput!: ValueControl;
  connectionIdInput!: ValueControl;
  connectionNameInput!: ValueControl;
  connectionUrlInput!: ValueControl;
  connectionFlavorSelect!: ValueControl;
  connectionApiKeyInput!: ValueControl;
  gatewayIdInput!: ValueControl;
  gatewayNameInput!: ValueControl;
  gatewayTypeSelect!: ValueControl;
  gatewayAgentSelect!: ValueControl;
  gatewayEnabledInput!: ValueControl;
  gatewayEnvInput!: ValueControl;
  gatewaySecretsInput!: ValueControl;
  kernelConfigHarnessSelect!: ValueControl;
  kernelConfigEnvInput!: ValueControl;
  gitAgentEnabledInput!: ValueControl;
  gitAgentRemoteInput!: ValueControl;
  gitAgentPatchInput!: ValueControl;
  gitAgentDefaultBranchInput!: ValueControl;
  gitAgentReviewerInput!: ValueControl;
  gitAgentValidationInput!: ValueControl;

  private started = false;
  private readonly intervals: Array<ReturnType<typeof window.setInterval>> = [];
  private logInterval: ReturnType<typeof window.setInterval> | null = null;
  private gatewayLogInterval: ReturnType<typeof window.setInterval> | null = null;
  private streamAbort: AbortController | null = null;
  private logSessionId = "";
  private gatewayLogId = "";

  connectedCallback(): void {
    super.connectedCallback();
    if (this.started) {
      return;
    }
    this.started = true;

    const start = (): void => this.startClientRuntime();
    if (document.readyState === "loading") {
      document.addEventListener("DOMContentLoaded", start, { once: true });
    } else {
      queueMicrotask(start);
    }
  }

  disconnectedCallback(): void {
    this.stopClientRuntime();
    super.disconnectedCallback();
  }

  navigate(view: ViewId): void {
    this.currentView = view;
    this.currentViewTitle = VIEW_META[view].title;
    this.currentViewDescription = VIEW_META[view].description;
    this.error = "";

    if (view === "config-kernels" && this.selectedKernelConfigHarness) {
      void this.loadKernelConfig(this.selectedKernelConfigHarness);
    }
    if (view === "git-agent") {
      void this.refreshGitAgent();
    }
  }

  toggleSidebar(): void {
    this.sidebarCollapsed = !this.sidebarCollapsed;
    localStorage.setItem("sidebar-collapsed", String(this.sidebarCollapsed));
  }

  toggleTheme(): void {
    this.darkMode = !this.darkMode;
    this.theme = this.darkMode ? "dark" : "light";
    localStorage.setItem("theme", this.theme);
  }

  clearError(): void {
    this.error = "";
  }

  async refreshAll(): Promise<void> {
    this.isRefreshing = true;
    try {
      const [
        harnesses,
        agents,
        workspaces,
        sessions,
        kernels,
        skills,
        connections,
        gateways,
        gatewayTypes,
        systemInfo,
        webuiInfo,
      ] = await Promise.all([
        this.loadOrDefault("harnesses", api.listHarnesses(), []),
        this.loadOrDefault("agents", api.listAgents(), []),
        this.loadOrDefault("workspaces", api.listWorkspaces(), []),
        this.loadOrDefault("sessions", api.listSessions(), []),
        this.loadOrDefault("kernels", api.listKernels(), []),
        this.loadOrDefault("skills", api.listSkills(), []),
        this.loadOrDefault("connections", api.listConnections(), []),
        this.loadOrDefault("gateways", api.listGateways(), []),
        this.loadOrDefault("gateway types", api.listGatewayTypes(), []),
        this.loadOrDefault<SystemInfo | null>("system info", api.getInfo(), null),
        this.loadOrDefault<ServiceInfoSection | null>("webui info", api.getWebuiInfo(), null),
      ]);

      this.harnesses = harnesses;
      this.agents = agents;
      this.workspaces = workspaces;
      this.sessions = normalizeSessions(sessions, agents);
      this.kernels = normalizeKernels(kernels);
      this.skills = skills;
      this.connections = connections;
      this.gateways = gateways;
      this.gatewayTypes = gatewayTypes;
      this.systemSections = createSystemSections({
        agentHost: systemInfo?.agent_host,
        clientService: systemInfo?.client_service,
        webui: webuiInfo ?? undefined,
      });
      this.generatedAtLabel = formatDate(new Date().toISOString());
      this.updateSummaryCards();
      this.ensureHarnessDefaults();
      if (this.currentView === "git-agent") {
        await this.refreshGitAgent();
      }
      if (this.selectedSessionId && !this.isStreaming) {
        await this.refreshSelectedSession();
      }
    } finally {
      this.isRefreshing = false;
    }
  }

  async selectSession(sessionId: string): Promise<void> {
    this.cancelStream();
    this.selectedSessionId = sessionId;
    this.navigate("chat");
    await this.refreshSelectedSession();
  }

  async createSession(): Promise<void> {
    const selectedAgent = controlValue(this.newSessionAgentSelect) || this.agents[0]?.agent_id;
    if (!selectedAgent) {
      this.reportError("Create an agent before starting a chat.");
      return;
    }
    try {
      const session = await api.createSession({
        agent_id: selectedAgent,
        channel_name: null,
        client_type: "webui",
      });
      await this.refreshSessions();
      await this.selectSession(session.session_id);
    } catch (error) {
      this.reportError(error);
    }
  }

  sendMessage(): void {
    if (!this.selectedSessionId) {
      this.reportError("Select a session before sending a message.");
      return;
    }
    const message = controlValue(this.composerInput).trim();
    if (!message) {
      return;
    }
    setControlValue(this.composerInput, "");
    this.cancelStream();
    const userMessage = normalizeChatMessage(createLocalMessage(this.selectedSessionId, "user", message));
    let assistantMessage = normalizeChatMessage(createLocalMessage(this.selectedSessionId, "assistant", ""));
    this.chatMessages = [...this.chatMessages, userMessage, assistantMessage];
    this.isStreaming = true;

    this.streamAbort = api.streamMessage(this.selectedSessionId, message, {
      onEvent: (event) => {
        assistantMessage = normalizeChatMessage(applyEventToAssistant(assistantMessage, event));
        this.replaceLastAssistant(assistantMessage);
      },
      onFinal: (chunk) => this.completeStream(chunk),
      onError: (error) => {
        this.isStreaming = false;
        this.reportError(error);
      },
    });
  }

  onComposerKeydown(event: KeyboardEvent): void {
    if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
      event.preventDefault();
      this.sendMessage();
    }
  }

  async resetSelectedSession(): Promise<void> {
    if (!this.selectedSessionId) {
      return;
    }
    if (!window.confirm("Reset this session? Conversation state in the kernel will be cleared.")) {
      return;
    }
    try {
      await api.resetSession(this.selectedSessionId);
      await this.refreshSelectedSession();
    } catch (error) {
      this.reportError(error);
    }
  }

  async saveSelectedWorkspace(): Promise<void> {
    if (!this.selectedSessionId) {
      return;
    }
    const details = promptWorkspaceSaveDetails();
    if (!details) {
      return;
    }
    try {
      await api.saveSessionWorkspace(this.selectedSessionId, details);
      await this.refreshWorkspaces();
    } catch (error) {
      this.reportError(error);
    }
  }

  async deleteSession(sessionId: string): Promise<void> {
    if (!window.confirm("Delete this session?")) {
      return;
    }
    try {
      await api.deleteSession(sessionId);
      if (this.selectedSessionId === sessionId) {
        this.selectedSessionId = "";
        this.selectedSessionTitle = "No session selected";
        this.chatMessages = [];
      }
      await this.refreshSessions();
    } catch (error) {
      this.reportError(error);
    }
  }

  showCreateAgent(): void {
    this.isEditingAgent = false;
    this.agentForm = emptyAgentForm(this.harnesses[0] ?? DEFAULT_HARNESS);
    this.showAgentForm = true;
  }

  editAgent(agentId: string): void {
    const agent = this.agents.find((item) => item.agent_id === agentId);
    if (!agent) {
      this.reportError(`Agent ${agentId} was not found.`);
      return;
    }
    this.isEditingAgent = true;
    this.agentForm = {
      agent_id: agent.agent_id,
      name: agent.name,
      harness: agent.harness,
      system_prompt: agent.system_prompt,
      skills_text: agent.skills.join(", "),
      env_vars: agent.env_vars,
      connection_id: agent.connection_id ?? "",
      workspace_mounts_json: JSON.stringify(
        agent.workspace_mounts.map((mount) => ({
          workspace_id: mount.workspace_id,
          mode: mount.mode,
        })),
        null,
        2,
      ),
    };
    this.showAgentForm = true;
  }

  cancelAgentForm(): void {
    this.showAgentForm = false;
  }

  async saveAgent(): Promise<void> {
    const form = this.readAgentForm();
    const mounts = parseWorkspaceMounts(form.workspace_mounts_json);
    if (!mounts.ok) {
      this.reportError(mounts.error);
      return;
    }
    try {
      if (this.isEditingAgent) {
        await api.updateAgent(form.agent_id, {
          name: form.name,
          harness: form.harness,
          system_prompt: form.system_prompt,
          skills: parseList(form.skills_text),
          env_vars: form.env_vars,
          connection_id: form.connection_id || null,
          workspace_mounts: mounts.value,
        });
      } else {
        await api.createAgent({
          agent_id: form.agent_id,
          name: form.name,
          harness: form.harness,
          system_prompt: form.system_prompt,
          skills: parseList(form.skills_text),
          env_vars: form.env_vars,
          connection_id: form.connection_id || null,
          workspace_mounts: mounts.value,
        });
      }
      this.showAgentForm = false;
      await this.refreshAgents();
    } catch (error) {
      this.reportError(error);
    }
  }

  async deleteAgent(agentId: string): Promise<void> {
    if (!window.confirm(`Delete agent ${agentId}?`)) {
      return;
    }
    try {
      await api.deleteAgent(agentId);
      await this.refreshAgents();
    } catch (error) {
      this.reportError(error);
    }
  }

  async startAgentSession(agentId: string): Promise<void> {
    try {
      const session = await api.createSession({
        agent_id: agentId,
        channel_name: null,
        client_type: "webui",
      });
      await this.refreshSessions();
      await this.selectSession(session.session_id);
    } catch (error) {
      this.reportError(error);
    }
  }

  toggleWorkspaceForm(): void {
    this.workspaceForm = emptyWorkspaceForm();
    this.showWorkspaceForm = !this.showWorkspaceForm;
  }

  async createWorkspace(): Promise<void> {
    const workspace_id = controlValue(this.workspaceIdInput).trim();
    const name = controlValue(this.workspaceNameInput).trim() || workspace_id;
    if (!workspace_id) {
      this.reportError("Workspace ID is required.");
      return;
    }
    try {
      await api.createWorkspace({ workspace_id, name });
      this.showWorkspaceForm = false;
      await this.refreshWorkspaces();
    } catch (error) {
      this.reportError(error);
    }
  }

  async renameWorkspace(workspaceId: string): Promise<void> {
    const current = this.workspaces.find((workspace) => workspace.workspace_id === workspaceId);
    const name = window.prompt("Workspace name", current?.name ?? workspaceId);
    if (!name) {
      return;
    }
    try {
      await api.updateWorkspace(workspaceId, { name });
      await this.refreshWorkspaces();
    } catch (error) {
      this.reportError(error);
    }
  }

  async cloneWorkspace(workspaceId: string): Promise<void> {
    const cloneId = window.prompt("New workspace ID", `${workspaceId}-copy`);
    if (!cloneId) {
      return;
    }
    const name = window.prompt("New workspace name", cloneId) ?? cloneId;
    try {
      await api.cloneWorkspace(workspaceId, { workspace_id: cloneId, name });
      await this.refreshWorkspaces();
    } catch (error) {
      this.reportError(error);
    }
  }

  async openWorkspace(workspaceId: string): Promise<void> {
    try {
      const result = await api.openWorkspaceVscode(workspaceId);
      openBrowserUrl(result.vscode_url);
    } catch (error) {
      this.reportError(error);
    }
  }

  async deleteWorkspace(workspaceId: string): Promise<void> {
    if (!window.confirm(`Delete workspace ${workspaceId}?`)) {
      return;
    }
    try {
      await api.deleteWorkspace(workspaceId);
      await this.refreshWorkspaces();
    } catch (error) {
      this.reportError(error);
    }
  }

  async openKernelLogs(sessionId: string, source: "harness" | "container"): Promise<void> {
    this.logSessionId = sessionId;
    this.logSource = source;
    this.logsTitle = `Kernel ${sessionId}`;
    this.showLogs = true;
    await this.refreshKernelLogs();
    if (this.logInterval !== null) {
      window.clearInterval(this.logInterval);
    }
    this.logInterval = window.setInterval(() => {
      void this.refreshKernelLogs();
    }, 1000);
  }

  closeLogs(): void {
    this.showLogs = false;
    this.logLines = [];
    this.logSessionId = "";
    if (this.logInterval !== null) {
      window.clearInterval(this.logInterval);
      this.logInterval = null;
    }
  }

  async downloadLogs(): Promise<void> {
    if (!this.logSessionId) {
      return;
    }
    try {
      const result =
        this.logSource === "container"
          ? await api.kernelContainerLogs(this.logSessionId, "all")
          : await api.kernelLogs(this.logSessionId);
      downloadText(`kernel-${this.logSessionId}-${this.logSource}.txt`, result.lines.join("\n"));
    } catch (error) {
      this.reportError(error);
    }
  }

  async killKernel(sessionId: string): Promise<void> {
    if (!window.confirm(`Stop kernel ${sessionId}?`)) {
      return;
    }
    try {
      await api.killKernel(sessionId);
      await this.refreshKernels();
    } catch (error) {
      this.reportError(error);
    }
  }

  openKernelUrl(url: string): void {
    openBrowserUrl(url);
  }

  newSkill(): void {
    this.selectedSkillId = "";
    this.skillVersions = [];
    this.skillForm = emptySkillForm();
    this.showSkillForm = true;
  }

  async selectSkill(skillId: string): Promise<void> {
    try {
      const [skill, versions] = await Promise.all([
        api.getSkill(skillId),
        api.listSkillVersions(skillId),
      ]);
      this.selectedSkillId = skillId;
      this.skillForm = {
        skill_id: skill.skill_id,
        files_json: JSON.stringify(skill.files ?? {}, null, 2),
      };
      this.skillVersions = versions;
      this.showSkillForm = true;
    } catch (error) {
      this.reportError(error);
    }
  }

  async saveSkill(): Promise<void> {
    const skillId = controlValue(this.skillIdInput).trim();
    const filesResult = parseJsonRecord(controlValue(this.skillFilesInput));
    if (!skillId) {
      this.reportError("Skill ID is required.");
      return;
    }
    if (!filesResult.ok) {
      this.reportError(filesResult.error);
      return;
    }
    try {
      if (this.selectedSkillId) {
        await api.updateSkill(skillId, filesResult.value);
      } else {
        await api.createSkill({ skill_id: skillId, files: filesResult.value });
      }
      await this.refreshSkills();
      await this.selectSkill(skillId);
    } catch (error) {
      this.reportError(error);
    }
  }

  async deleteSkill(): Promise<void> {
    const skillId = this.selectedSkillId || controlValue(this.skillIdInput).trim();
    if (!skillId || !window.confirm(`Delete skill ${skillId}?`)) {
      return;
    }
    try {
      await api.deleteSkill(skillId);
      this.showSkillForm = false;
      this.selectedSkillId = "";
      this.skillVersions = [];
      await this.refreshSkills();
    } catch (error) {
      this.reportError(error);
    }
  }

  async rollbackSkillVersion(version: number): Promise<void> {
    if (!this.selectedSkillId) {
      return;
    }
    try {
      await api.rollbackSkillVersion(this.selectedSkillId, version);
      await this.selectSkill(this.selectedSkillId);
    } catch (error) {
      this.reportError(error);
    }
  }

  showCreateConnection(): void {
    this.isEditingConnection = false;
    this.connectionForm = emptyConnectionForm();
    this.showConnectionForm = true;
  }

  editConnection(connectionId: string): void {
    const connection = this.connections.find((item) => item.connection_id === connectionId);
    if (!connection) {
      this.reportError(`Connection ${connectionId} was not found.`);
      return;
    }
    this.isEditingConnection = true;
    this.connectionForm = {
      connection_id: connection.connection_id,
      name: connection.name,
      url: connection.url,
      api_flavor: connection.api_flavor,
      api_key: "",
    };
    this.showConnectionForm = true;
  }

  cancelConnectionForm(): void {
    this.showConnectionForm = false;
  }

  async saveConnection(): Promise<void> {
    const form = this.readConnectionForm();
    if (!form.connection_id || !form.name || !form.url) {
      this.reportError("Connection ID, name, and URL are required.");
      return;
    }
    try {
      if (this.isEditingConnection) {
        await api.updateConnection(form.connection_id, {
          name: form.name,
          url: form.url,
          api_flavor: form.api_flavor,
          api_key: form.api_key,
        });
      } else {
        await api.createConnection(form);
      }
      this.showConnectionForm = false;
      await this.refreshConnections();
    } catch (error) {
      this.reportError(error);
    }
  }

  async deleteConnection(connectionId: string): Promise<void> {
    if (!window.confirm(`Delete connection ${connectionId}?`)) {
      return;
    }
    try {
      await api.deleteConnection(connectionId);
      await this.refreshConnections();
    } catch (error) {
      this.reportError(error);
    }
  }

  async loadConnectionModels(connectionId: string): Promise<void> {
    try {
      const models = await api.listConnectionModels(connectionId);
      this.selectedConnectionModelsText = JSON.stringify(models, null, 2);
    } catch (error) {
      this.reportError(error);
    }
  }

  showCreateGateway(): void {
    this.isEditingGateway = false;
    this.gatewayForm = emptyGatewayForm(this.gatewayTypes[0] ?? "", this.agents[0]?.agent_id ?? "");
    this.showGatewayForm = true;
  }

  editGateway(gatewayId: string): void {
    const gateway = this.gateways.find((item) => item.gateway_id === gatewayId);
    if (!gateway) {
      this.reportError(`Gateway ${gatewayId} was not found.`);
      return;
    }
    this.isEditingGateway = true;
    this.gatewayForm = {
      gateway_id: gateway.gateway_id,
      name: gateway.name,
      gateway_type: gateway.gateway_type,
      agent_id: gateway.agent_id,
      enabled: gateway.enabled,
      env_vars: gateway.env_vars,
      secrets_json: "{}",
    };
    this.showGatewayForm = true;
  }

  cancelGatewayForm(): void {
    this.showGatewayForm = false;
  }

  async saveGateway(): Promise<void> {
    const form = this.readGatewayForm();
    const secrets = parseJsonRecord(controlValue(this.gatewaySecretsInput));
    if (!secrets.ok) {
      this.reportError(secrets.error);
      return;
    }
    try {
      if (this.isEditingGateway) {
        await api.updateGateway(form.gateway_id, {
          name: form.name,
          agent_id: form.agent_id,
          enabled: form.enabled,
          env_vars: form.env_vars,
          secrets: secrets.value,
        });
      } else {
        await api.createGateway({
          gateway_id: form.gateway_id,
          name: form.name,
          gateway_type: form.gateway_type,
          agent_id: form.agent_id,
          enabled: form.enabled,
          env_vars: form.env_vars,
          secrets: secrets.value,
        });
      }
      this.showGatewayForm = false;
      await this.refreshGateways();
    } catch (error) {
      this.reportError(error);
    }
  }

  async deleteGateway(gatewayId: string): Promise<void> {
    if (!window.confirm(`Delete gateway ${gatewayId}?`)) {
      return;
    }
    try {
      await api.deleteGateway(gatewayId);
      await this.refreshGateways();
    } catch (error) {
      this.reportError(error);
    }
  }

  async startGateway(gatewayId: string): Promise<void> {
    try {
      await api.startGateway(gatewayId);
      await this.refreshGateways();
    } catch (error) {
      this.reportError(error);
    }
  }

  async stopGateway(gatewayId: string): Promise<void> {
    try {
      await api.stopGateway(gatewayId);
      await this.refreshGateways();
    } catch (error) {
      this.reportError(error);
    }
  }

  async openGatewayLogs(gatewayId: string): Promise<void> {
    this.gatewayLogId = gatewayId;
    this.gatewayLogsTitle = `Gateway ${gatewayId}`;
    this.showGatewayLogs = true;
    await this.refreshGatewayLogs();
    if (this.gatewayLogInterval !== null) {
      window.clearInterval(this.gatewayLogInterval);
    }
    this.gatewayLogInterval = window.setInterval(() => {
      void this.refreshGatewayLogs();
    }, 3000);
  }

  closeGatewayLogs(): void {
    this.showGatewayLogs = false;
    this.gatewayLogLines = [];
    this.gatewayLogId = "";
    if (this.gatewayLogInterval !== null) {
      window.clearInterval(this.gatewayLogInterval);
      this.gatewayLogInterval = null;
    }
  }

  async selectKernelConfigHarness(): Promise<void> {
    const harness = controlValue(this.kernelConfigHarnessSelect);
    if (!harness) {
      return;
    }
    this.selectedKernelConfigHarness = harness;
    await this.loadKernelConfig(harness);
  }

  async saveKernelConfig(): Promise<void> {
    const harness = controlValue(this.kernelConfigHarnessSelect) || this.selectedKernelConfigHarness;
    if (!harness) {
      this.reportError("Select a harness before saving kernel config.");
      return;
    }
    try {
      await api.updateKernelConfig(harness, controlValue(this.kernelConfigEnvInput));
      await this.loadKernelConfig(harness);
    } catch (error) {
      this.reportError(error);
    }
  }

  async saveGitAgentConfig(): Promise<void> {
    const enabled = Boolean(this.gitAgentEnabledInput?.checked);
    try {
      await api.updateGitAgentConfig({
        enabled,
        remote_url: controlValue(this.gitAgentRemoteInput),
        patch_url: controlValue(this.gitAgentPatchInput),
        default_branch: controlValue(this.gitAgentDefaultBranchInput),
        review_agent_id: controlValue(this.gitAgentReviewerInput),
        validation_command: controlValue(this.gitAgentValidationInput),
      });
      await this.refreshGitAgent();
    } catch (error) {
      this.reportError(error);
    }
  }

  async selectGitRequest(requestId: string): Promise<void> {
    if (!requestId) {
      return;
    }
    try {
      const request = await api.getGitAgentRequest(requestId);
      this.selectedGitRequest = normalizeGitRequestDetail(request);
    } catch (error) {
      this.reportError(error);
    }
  }

  private startClientRuntime(): void {
    const storedTheme = localStorage.getItem("theme");
    const prefersDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
    this.darkMode = storedTheme ? storedTheme === "dark" : prefersDark;
    this.theme = this.darkMode ? "dark" : "light";
    this.sidebarCollapsed = localStorage.getItem("sidebar-collapsed") === "true";
    this.ensureHarnessDefaults();
    this.updateSummaryCards();

    void this.refreshAll();
    this.intervals.push(
      window.setInterval(() => {
        void this.refreshSessions();
      }, 5000),
      window.setInterval(() => {
        void this.refreshKernels();
      }, 2000),
      window.setInterval(() => {
        void this.refreshGateways();
        if (this.currentView === "git-agent") {
          void this.refreshGitAgent();
        }
      }, 5000),
    );
  }

  private stopClientRuntime(): void {
    this.cancelStream();
    for (const interval of this.intervals) {
      window.clearInterval(interval);
    }
    this.intervals.length = 0;
    this.closeLogs();
    this.closeGatewayLogs();
  }

  private async refreshAgents(): Promise<void> {
    const agents = await this.loadOrDefault("agents", api.listAgents(), this.agents);
    this.agents = agents;
    await this.refreshSessions();
    this.updateSummaryCards();
  }

  private async refreshWorkspaces(): Promise<void> {
    this.workspaces = await this.loadOrDefault("workspaces", api.listWorkspaces(), this.workspaces);
    this.updateSummaryCards();
  }

  private async refreshSessions(): Promise<void> {
    const sessions = await this.loadOrDefault("sessions", api.listSessions(), []);
    this.sessions = normalizeSessions(sessions, this.agents);
    this.updateSummaryCards();
    if (this.selectedSessionId && !this.isStreaming) {
      await this.refreshSelectedSession();
    }
  }

  private async refreshSelectedSession(): Promise<void> {
    if (!this.selectedSessionId) {
      return;
    }
    try {
      const detail = await api.getSession(this.selectedSessionId);
      const agent = this.agents.find((item) => item.agent_id === detail.agent_id);
      this.selectedSessionTitle = `${agent?.name ?? detail.agent_id} / ${compactId(detail.session_id)}`;
      this.chatMessages = detail.messages.map(normalizeChatMessage);
      if (detail.active_turn && !this.isStreaming) {
        this.resumeTurn(detail.active_turn.turn_id);
      }
    } catch (error) {
      this.reportError(error);
    }
  }

  private async refreshKernels(): Promise<void> {
    this.kernels = normalizeKernels(await this.loadOrDefault("kernels", api.listKernels(), []));
    this.updateSummaryCards();
  }

  private async refreshKernelLogs(): Promise<void> {
    if (!this.logSessionId) {
      return;
    }
    try {
      const result =
        this.logSource === "container"
          ? await api.kernelContainerLogs(this.logSessionId)
          : await api.kernelLogs(this.logSessionId);
      this.logLines = result.lines;
    } catch (error) {
      this.reportError(error);
    }
  }

  private async refreshSkills(): Promise<void> {
    this.skills = await this.loadOrDefault("skills", api.listSkills(), this.skills);
  }

  private async refreshConnections(): Promise<void> {
    this.connections = await this.loadOrDefault("connections", api.listConnections(), this.connections);
  }

  private async refreshGateways(): Promise<void> {
    this.gateways = await this.loadOrDefault("gateways", api.listGateways(), this.gateways);
    this.updateSummaryCards();
  }

  private async refreshGatewayLogs(): Promise<void> {
    if (!this.gatewayLogId) {
      return;
    }
    try {
      const result = await api.gatewayLogs(this.gatewayLogId);
      this.gatewayLogLines = result.lines;
    } catch (error) {
      this.reportError(error);
    }
  }

  private async refreshGitAgent(): Promise<void> {
    const [status, requests, config] = await Promise.all([
      this.loadOrDefault<GitAgentStatus | null>("git agent status", api.getGitAgentStatus(), null),
      this.loadOrDefault<GitAgentRequestsResponse>("git agent requests", api.listGitAgentRequests(), []),
      this.loadOrDefault<GitAgentConfig | null>("git agent config", api.getGitAgentConfig(), null),
    ]);
    this.gitAgentStatusRows = createStatusRows(status);
    this.gitAgentRequests = normalizeGitRequests(extractGitRequests(requests));
    if (config) {
      this.gitAgentConfig = toGitAgentConfigForm(config);
    }
  }

  private async loadKernelConfig(harness: string): Promise<void> {
    try {
      const config = await api.getKernelConfig(harness);
      this.selectedKernelConfigHarness = harness;
      this.kernelConfigEnv = config.env_vars;
    } catch (error) {
      this.reportError(error);
    }
  }

  private async loadOrDefault<T>(label: string, promise: Promise<T>, fallback: T): Promise<T> {
    try {
      return await promise;
    } catch (error) {
      this.reportError(`${label}: ${toErrorMessage(error)}`);
      return fallback;
    }
  }

  private updateSummaryCards(): void {
    this.summaryCards = createSummaryCards({
      agents: this.agents,
      workspaces: this.workspaces,
      sessions: this.sessions,
      kernels: this.kernels,
      gateways: this.gateways,
    });
  }

  private ensureHarnessDefaults(): void {
    if (!this.selectedKernelConfigHarness) {
      this.selectedKernelConfigHarness = this.harnesses[0] ?? DEFAULT_HARNESS;
    }
    if (!this.agentForm.harness) {
      this.agentForm = { ...this.agentForm, harness: this.harnesses[0] ?? DEFAULT_HARNESS };
    }
  }

  private readAgentForm(): AgentFormState {
    return {
      agent_id: controlValue(this.agentIdInput).trim(),
      name: controlValue(this.agentNameInput).trim(),
      harness: controlValue(this.agentHarnessSelect) || DEFAULT_HARNESS,
      system_prompt: controlValue(this.agentPromptInput) || DEFAULT_AGENT_SYSTEM_PROMPT,
      skills_text: controlValue(this.agentSkillsInput),
      env_vars: controlValue(this.agentEnvInput),
      connection_id: controlValue(this.agentConnectionSelect),
      workspace_mounts_json: controlValue(this.agentMountsInput) || "[]",
    };
  }

  private readConnectionForm(): ConnectionFormState {
    const apiFlavor = controlValue(this.connectionFlavorSelect);
    return {
      connection_id: controlValue(this.connectionIdInput).trim(),
      name: controlValue(this.connectionNameInput).trim(),
      url: controlValue(this.connectionUrlInput).trim(),
      api_flavor: apiFlavor === "responses" ? "responses" : "chat_completions",
      api_key: controlValue(this.connectionApiKeyInput),
    };
  }

  private readGatewayForm(): GatewayFormState {
    return {
      gateway_id: controlValue(this.gatewayIdInput).trim(),
      name: controlValue(this.gatewayNameInput).trim(),
      gateway_type: controlValue(this.gatewayTypeSelect),
      agent_id: controlValue(this.gatewayAgentSelect),
      enabled: Boolean(this.gatewayEnabledInput?.checked),
      env_vars: controlValue(this.gatewayEnvInput),
      secrets_json: controlValue(this.gatewaySecretsInput),
    };
  }

  private completeStream(chunk: MessageStreamFinalChunk): void {
    this.isStreaming = false;
    this.streamAbort = null;
    let assistantMessage = normalizeChatMessage(chunk.assistant_message);
    if (chunk.error) {
      assistantMessage = {
        ...assistantMessage,
        content: assistantMessage.content || `Stream failed: ${chunk.error}`,
      };
      this.reportError(chunk.error);
    }
    this.chatMessages = replaceAssistantMessage(this.chatMessages, assistantMessage);
    void this.refreshSessions();
  }

  private resumeTurn(turnId: string): void {
    if (!this.selectedSessionId || this.isStreaming) {
      return;
    }
    this.isStreaming = true;
    let assistantMessage =
      lastAssistantMessage(this.chatMessages) ??
      normalizeChatMessage(createLocalMessage(this.selectedSessionId, "assistant", ""));
    if (!this.chatMessages.some((message) => message.message_id === assistantMessage.message_id)) {
      this.chatMessages = [...this.chatMessages, assistantMessage];
    }
    this.streamAbort = api.streamTurn(this.selectedSessionId, turnId, {
      onEvent: (event) => {
        assistantMessage = normalizeChatMessage(applyEventToAssistant(assistantMessage, event));
        this.replaceLastAssistant(assistantMessage);
      },
      onFinal: (chunk) => this.completeStream(chunk),
      onError: (error) => {
        this.isStreaming = false;
        this.reportError(error);
      },
    });
  }

  private replaceLastAssistant(message: UiChatMessage): void {
    this.chatMessages = replaceAssistantMessage(this.chatMessages, message);
  }

  private cancelStream(): void {
    if (this.streamAbort) {
      this.streamAbort.abort();
      this.streamAbort = null;
    }
    this.isStreaming = false;
  }

  private reportError(error: unknown): void {
    this.error = toErrorMessage(error);
  }
}

AgentspaceApp.define("agentspace-app");

function controlValue(control: ValueControl | undefined): string {
  return typeof control?.value === "string" ? control.value : "";
}

function setControlValue(control: ValueControl | undefined, value: string): void {
  if (control) {
    control.value = value;
  }
}

function parseList(value: string): string[] {
  return value
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean);
}

function parseWorkspaceMounts(value: string): { ok: true; value: WorkspaceMountInput[] } | { ok: false; error: string } {
  try {
    const parsed = JSON.parse(value || "[]") as unknown;
    if (!Array.isArray(parsed)) {
      return { ok: false, error: "Workspace mounts must be a JSON array." };
    }
    const mounts: WorkspaceMountInput[] = [];
    for (const item of parsed) {
      if (!isRecord(item) || typeof item.workspace_id !== "string") {
        return { ok: false, error: "Each workspace mount needs a workspace_id string." };
      }
      const mode = item.mode === "ro" ? "ro" : "rw";
      mounts.push({ workspace_id: item.workspace_id, mode });
    }
    return { ok: true, value: mounts };
  } catch (error) {
    return { ok: false, error: `Invalid workspace mounts JSON: ${toErrorMessage(error)}` };
  }
}

function parseJsonRecord(value: string): { ok: true; value: Record<string, string> } | { ok: false; error: string } {
  try {
    const parsed = JSON.parse(value || "{}") as unknown;
    if (!isRecord(parsed)) {
      return { ok: false, error: "Expected a JSON object." };
    }
    const record: Record<string, string> = {};
    for (const [key, entryValue] of Object.entries(parsed)) {
      record[key] = typeof entryValue === "string" ? entryValue : JSON.stringify(entryValue);
    }
    return { ok: true, value: record };
  } catch (error) {
    return { ok: false, error: `Invalid JSON: ${toErrorMessage(error)}` };
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function extractGitRequests(response: GitAgentRequestsResponse): Parameters<typeof normalizeGitRequests>[0] {
  if (Array.isArray(response)) {
    return response;
  }
  return response.requests ?? response.patch_requests ?? response.items ?? response.data ?? [];
}

function toGitAgentConfigForm(config: GitAgentConfig): GitAgentConfigFormState {
  const policy = config.policy ?? config;
  return {
    enabled: Boolean(config.enabled),
    remote_url: config.remote_url ?? "",
    patch_url: config.patch_url ?? "",
    default_branch: config.default_branch ?? "",
    review_agent_id: config.review_agent_id ?? config.reviewer_agent_id ?? "",
    validation_command: config.validation_command ?? "",
    allowed_refs: joinList(policy.allowed_refs),
    allowed_ref_prefixes: joinList(policy.allowed_ref_prefixes),
    protected_refs: joinList(policy.protected_refs),
    protected_ref_prefixes: joinList(policy.protected_ref_prefixes),
    skip_review_refs: joinList(policy.skip_review_refs),
    skip_validation_refs: joinList(policy.skip_validation_refs),
  };
}

function joinList(values?: string[] | null): string {
  return values?.join("\n") ?? "";
}

function createLocalMessage(
  sessionId: string,
  role: "user" | "assistant",
  content: string,
): ChatMessage {
  return {
    message_id: `${role}-${globalThis.crypto.randomUUID()}`,
    session_id: sessionId,
    role,
    content,
    created_at: new Date().toISOString(),
    tool_calls: [],
  };
}

function normalizeChatMessage(message: ChatMessage): UiChatMessage {
  return {
    ...message,
    role_label: message.role === "assistant" ? "Assistant" : message.role === "user" ? "You" : message.role,
    created_label: formatDate(message.created_at),
    tool_calls: (message.tool_calls ?? []).map(normalizeToolCall),
  };
}

function normalizeToolCall(toolCall: UiToolCall): UiToolCall;
function normalizeToolCall(toolCall: NonNullable<ChatMessage["tool_calls"]>[number]): UiToolCall;
function normalizeToolCall(toolCall: NonNullable<ChatMessage["tool_calls"]>[number]): UiToolCall {
  return {
    ...toolCall,
    input_text: toolCall.input ?? "",
    output_text: toolCall.output ?? "",
    status_label: toolCall.status ?? toolCall.kind ?? "",
  };
}

function applyEventToAssistant(message: UiChatMessage, event: KernelEvent): UiChatMessage {
  if (event.type === "session/update" && event.update) {
    return applyAcpUpdateToAssistant(message, event.update);
  }
  if (event.type === "text_delta" && event.content) {
    return { ...message, content: `${message.content}${event.content}` };
  }
  if (event.type === "reasoning_delta" && event.content) {
    return { ...message, reasoning: `${message.reasoning ?? ""}${event.content}` };
  }
  if (event.type === "tool_call" && event.tool) {
    return {
      ...message,
      tool_calls: [
        ...message.tool_calls,
        normalizeToolCall({
          tool: event.tool,
          input: event.input ? JSON.stringify(event.input, null, 2) : undefined,
          content_offset: message.content.trim().length,
        }),
      ],
    };
  }
  if (event.type === "tool_result" && event.tool && event.output !== null && event.output !== undefined) {
    const toolCalls = [...message.tool_calls];
    const index = toolCalls.findIndex((toolCall) => toolCall.tool === event.tool && !toolCall.output);
    if (index >= 0) {
      toolCalls[index] = normalizeToolCall({ ...toolCalls[index], output: event.output });
      return { ...message, tool_calls: toolCalls };
    }
  }
  return message;
}

function applyAcpUpdateToAssistant(message: UiChatMessage, update: AcpSessionUpdate): UiChatMessage {
  if (update.sessionUpdate === "agent_message_chunk") {
    return { ...message, content: `${message.content}${contentText(update.content)}` };
  }
  if (update.sessionUpdate === "agent_thought_chunk") {
    return { ...message, reasoning: `${message.reasoning ?? ""}${contentText(update.content)}` };
  }
  if (update.sessionUpdate === "plan") {
    return {
      ...message,
      reasoning: `${message.reasoning ?? ""}${JSON.stringify({ plan: update.entries }, null, 2)}`,
    };
  }
  if (update.sessionUpdate === "tool_call" || update.sessionUpdate === "tool_call_update") {
    return upsertToolCall(message, update);
  }
  return message;
}

function upsertToolCall(message: UiChatMessage, update: AcpSessionUpdate): UiChatMessage {
  const toolCallId = typeof update.toolCallId === "string" ? update.toolCallId : undefined;
  const toolCalls = [...message.tool_calls];
  let index = toolCallId ? toolCalls.findIndex((toolCall) => toolCall.tool_call_id === toolCallId) : -1;
  if (index < 0) {
    toolCalls.push(
      normalizeToolCall({
        tool: typeof update.title === "string" && update.title ? update.title : toolCallId ?? "tool",
        tool_call_id: toolCallId,
        content_offset: message.content.trim().length,
      }),
    );
    index = toolCalls.length - 1;
  }
  const current = toolCalls[index];
  toolCalls[index] = normalizeToolCall({
    ...current,
    tool: typeof update.title === "string" && update.title ? update.title : current.tool,
    status: typeof update.status === "string" ? update.status : current.status,
    kind: typeof update.kind === "string" ? update.kind : current.kind,
    input: Object.hasOwn(update, "rawInput") ? jsonText(update.rawInput) : current.input,
    output: toolOutput(update) ?? current.output,
  });
  return { ...message, tool_calls: toolCalls };
}

function toolOutput(update: AcpSessionUpdate): string | undefined {
  if (Object.hasOwn(update, "rawOutput")) {
    return jsonText(update.rawOutput);
  }
  const text = contentText(update.content);
  return text || undefined;
}

function jsonText(value: unknown): string | undefined {
  if (value === null || value === undefined) {
    return undefined;
  }
  return typeof value === "string" ? value : JSON.stringify(value, null, 2);
}

function contentText(content: unknown): string {
  if (Array.isArray(content)) {
    return content.map(contentText).join("");
  }
  if (content === null || content === undefined) {
    return "";
  }
  if (typeof content === "string") {
    return content;
  }
  if (
    typeof content === "number" ||
    typeof content === "boolean" ||
    typeof content === "bigint"
  ) {
    return content.toString();
  }
  if (typeof content !== "object") {
    return "";
  }
  const block = content as Record<string, unknown>;
  if (block.type === "text") {
    return typeof block.text === "string" ? block.text : "";
  }
  if (block.type === "content") {
    return contentText(block.content);
  }
  return JSON.stringify(block);
}

function replaceAssistantMessage(messages: UiChatMessage[], assistant: UiChatMessage): UiChatMessage[] {
  const index = lastAssistantIndex(messages);
  if (index < 0) {
    return [...messages, assistant];
  }
  return messages.map((message, messageIndex) => (messageIndex === index ? assistant : message));
}

function lastAssistantMessage(messages: UiChatMessage[]): UiChatMessage | undefined {
  const index = lastAssistantIndex(messages);
  return index >= 0 ? messages[index] : undefined;
}

function lastAssistantIndex(messages: UiChatMessage[]): number {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    if (messages[index].role === "assistant") {
      return index;
    }
  }
  return -1;
}

function promptWorkspaceSaveDetails(): { workspace_id: string; name: string } | null {
  const workspace_id = window.prompt("Workspace ID to save");
  if (!workspace_id) {
    return null;
  }
  const name = window.prompt("Workspace name", workspace_id);
  return { workspace_id, name: name || workspace_id };
}

function openBrowserUrl(url: string | null | undefined): void {
  const normalized = browserReachableLocalUrl(url);
  if (!normalized) {
    return;
  }
  window.open(normalized, "_blank", "noopener,noreferrer");
}

function browserReachableLocalUrl(url: string | null | undefined): string {
  if (!url) {
    return "";
  }
  try {
    const parsed = new URL(url);
    if (parsed.hostname === "0.0.0.0" || parsed.hostname === "127.0.0.1") {
      parsed.hostname = window.location.hostname || "127.0.0.1";
    }
    return parsed.toString();
  } catch {
    return url;
  }
}

function downloadText(filename: string, content: string): void {
  const blob = new Blob([content], { type: "text/plain;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  document.body.appendChild(link);
  link.click();
  document.body.removeChild(link);
  URL.revokeObjectURL(url);
}

function compactId(value: string): string {
  return value.length > 12 ? value.slice(0, 12) : value;
}

function toErrorMessage(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }
  return String(error);
}
