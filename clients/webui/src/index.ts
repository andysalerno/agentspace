import {
  fluentBadge,
  fluentButton,
  fluentCard,
  fluentCheckbox,
  fluentDesignSystemProvider,
  fluentDivider,
  fluentOption,
  fluentProgressRing,
  fluentSelect,
  fluentSwitch,
  fluentTextArea,
  fluentTextField,
  provideFluentDesignSystem,
} from "@fluentui/web-components";

import "./agentspace-app/agentspace-app";
import "./monaco-text-editor";

provideFluentDesignSystem().register(
  fluentDesignSystemProvider(),
  fluentBadge(),
  fluentButton(),
  fluentCard(),
  fluentCheckbox(),
  fluentDivider(),
  fluentOption(),
  fluentProgressRing(),
  fluentSelect(),
  fluentSwitch(),
  fluentTextArea(),
  fluentTextField(),
);
