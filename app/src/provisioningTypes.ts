// Phase 80 (unified setup wizard): shared type mirrors for the two
// step-engine flows — remote provisioning (ProvisionNewServerFlow,
// events provisioning:progress/:complete) and local smart setup
// (LocalSetupFlow, events local-setup:progress/:complete). Both engines
// emit the SAME StepProgress payload shape; the Rust source of truth is
// src-tauri/src/provisioning.rs.

// Phase 32.A: structured error variant carried alongside `message`.
// When present, drives a dedicated UI (sudo-required modal, step
// stderr block) instead of the generic message line.
export type ProvisioningError =
  | { kind: "SudoRequired"; details: { user: string; raw_stderr: string } }
  | {
      kind: "StepFailed";
      details: { step: string; exit_code: number; stderr: string };
    }
  // Phase 80: a local install step needs elevation and/or a reboot
  // (WSL feature enable). The wizard renders an instruction card.
  | { kind: "ElevationRequired"; details: { step: string; hint: string } }
  // Windows servicing returned 3010: the features are staged but only
  // come alive after a restart, so retrying without one cannot work.
  | { kind: "RebootRequired"; details: { step: string; hint: string } }
  | { kind: "Generic"; details: string };

export interface StepProgress {
  run_id: string;
  step_index: number;
  step_kind: string;
  state: "running" | "done" | "failed" | "skipped";
  log_chunk: string;
  message?: string | null;
  error?: ProvisioningError | null;
  timestamp_iso: string;
}

export interface RunHandle {
  run_id: string;
}
