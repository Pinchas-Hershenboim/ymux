import { createSignal } from "solid-js";
import { t } from "./i18n";
import type { CreateWorkspaceInput, EnvVar } from "./types";
import {
  createSshFormState,
  buildSshConnection,
  SshConnectionFields,
} from "./SshConnectionFields";
import { WorkspaceExtrasFields } from "./WorkspaceExtrasFields";

// Phase 80 (unified setup wizard): "server → existing → I already have a
// key" — the quick-SSH create experience that used to live on
// CreateWorkspaceModal's SSH tab. Records connection details (ssh-config
// import, key picker + perms fix, test connection) and creates the
// workspace; it does NOT touch the remote host. The password→smart-key
// sibling path is ConnectExistingFlow.

interface Props {
  onCreate: (input: CreateWorkspaceInput) => void;
  onClose: () => void;
  // Phase 34: open the SSH-key-setup HelpPane as a side-by-side split.
  onOpenSshHelp?: () => void;
}

export function QuickSshCreateFlow(p: Props) {
  const [name, setName] = createSignal("");
  const [color, setColor] = createSignal("#7aa2f7");
  const [setupCmd, setSetupCmd] = createSignal("");
  const [teardownCmd, setTeardownCmd] = createSignal("");
  const [envRows, setEnvRows] = createSignal<EnvVar[]>([]);
  const ssh = createSshFormState();

  const cleanedEnv = (): EnvVar[] =>
    envRows().filter((r) => r.key.trim() !== "");

  const submit = () => {
    if (!name().trim()) return;
    const connection = buildSshConnection(ssh);
    if (!connection) return; // host/user required
    p.onCreate({
      name: name().trim(),
      connection,
      color: color(),
      setup_command: setupCmd() || undefined,
      teardown_command: teardownCmd() || undefined,
      env: cleanedEnv().length ? cleanedEnv() : undefined,
    });
    p.onClose();
  };

  return (
    <>
      <label>
        <span>{t("ws.create.field.name")}</span>
        <input
          autofocus
          value={name()}
          onInput={(e) => setName(e.currentTarget.value)}
          placeholder={t("ws.create.field.name.placeholder")}
        />
      </label>

      <label>
        <span>{t("ws.create.field.color")}</span>
        <input
          type="color"
          value={color()}
          onInput={(e) => setColor(e.currentTarget.value)}
        />
      </label>

      <SshConnectionFields
        state={ssh}
        showImport={true}
        onOpenSshHelp={p.onOpenSshHelp}
        onImportName={(alias) => {
          // Seed the workspace name from the imported host alias — only
          // if the user hasn't typed their own yet.
          if (!name().trim()) setName(alias);
        }}
      />

      <hr class="modal-sep" />

      <WorkspaceExtrasFields
        setupCmd={setupCmd}
        setSetupCmd={setSetupCmd}
        teardownCmd={teardownCmd}
        setTeardownCmd={setTeardownCmd}
        envRows={envRows}
        setEnvRows={setEnvRows}
      />

      <div class="modal-buttons">
        <button onClick={p.onClose}>{t("common.cancel")}</button>
        <button
          class="primary"
          disabled={!name().trim() || !ssh.host().trim() || !ssh.user().trim()}
          onClick={submit}
        >
          {t("ws.create.btn.create")}
        </button>
      </div>
    </>
  );
}
