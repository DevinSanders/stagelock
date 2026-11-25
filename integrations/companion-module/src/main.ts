import {
  InstanceBase,
  InstanceStatus,
  combineRgb,
  type SomeCompanionConfigField,
  type CompanionActionDefinitions,
  type CompanionFeedbackDefinitions,
  type CompanionPresetDefinitions,
  type CompanionPresetSection,
  type CompanionStaticUpgradeScript,
  type CompanionVariableDefinitions,
} from "@companion-module/base";
import WebSocket from "ws";

type ModuleConfig = {
  host: string;
  port: number;
};

type ActionsSchema = {
  toggle_track: { options: { camera_index: number } };
  set_framing_preset: { options: { camera_index: number; preset: string } };
  set_thirds: { options: { camera_index: number; thirds: string } };
  one_push_af: { options: { camera_index: number } };
  recall_preset: { options: { camera_index: number; slot: number } };
  toggle_ndi: { options: { camera_index: number } };
  dial_zoom: { options: { camera_index: number; delta: number } };
  dial_exposure: { options: { camera_index: number; delta: number } };
  vmix_cut: { options: { input: string } };
  vmix_fade: { options: { input: string; duration_ms: number } };
};

type FeedbacksSchema = {
  tracking_state: { type: "boolean"; options: { camera_index: number } };
  tally_state: { type: "boolean"; options: { camera_index: number } };
  framing_preset: { type: "boolean"; options: { camera_index: number; preset: string } };
  ndi_active: { type: "boolean"; options: { camera_index: number } };
};

type VariablesSchema = {
  [variableId: string]: string | number | undefined;
};

export type ModuleSchema = {
  config: ModuleConfig;
  secrets: undefined;
  actions: ActionsSchema;
  feedbacks: FeedbacksSchema;
  variables: VariablesSchema;
};

interface TelemetryCam {
  camera_id: string;
  ndi_output_active: boolean;
  tally: "off" | "preview" | "program";
  tracking_state: "idle" | "locked" | "occluded_hold" | "searching";
  auto_track_enabled: boolean;
  framing_preset: "close_up" | "waist_up" | "full_height";
  rule_of_thirds: "center" | "left_third" | "right_third" | "auto_lead_room";
  focus_sharpness: number;
}

export const UpgradeScripts: CompanionStaticUpgradeScript<ModuleConfig, undefined>[] = [];

export default class StageLockInstance extends InstanceBase<ModuleSchema> {
  private config: ModuleConfig = { host: "127.0.0.1", port: 8765 };
  private ws: WebSocket | null = null;
  private reconnectTimer: NodeJS.Timeout | null = null;
  private cameras: TelemetryCam[] = [];

  async init(config: ModuleConfig): Promise<void> {
    this.config = {
      host: config.host || "127.0.0.1",
      port: Number(config.port) || 8765,
    };

    this.setActionDefinitions(this.buildActions());
    this.setFeedbackDefinitions(this.buildFeedbacks());
    this.setPresetDefinitions(this.buildPresetSections(), this.buildPresets());
    this.setVariableDefinitions(this.buildVariables());

    this.connectWebSocket();
  }

  async destroy(): Promise<void> {
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.ws?.close();
    this.ws = null;
  }

  async configUpdated(config: ModuleConfig): Promise<void> {
    this.config = {
      host: config.host || "127.0.0.1",
      port: Number(config.port) || 8765,
    };
    this.ws?.close();
    this.connectWebSocket();
  }

  getConfigFields(): SomeCompanionConfigField[] {
    return [
      {
        type: "textinput",
        id: "host",
        label: "StageLock Host / IP",
        width: 8,
        default: "127.0.0.1",
      },
      {
        type: "number",
        id: "port",
        label: "HTTP / WS Port",
        width: 4,
        min: 1,
        max: 65535,
        default: 8765,
      },
    ];
  }

  private baseUrl(): string {
    return `http://${this.config.host}:${this.config.port}`;
  }

  private async postSurfaceAction(action: Record<string, unknown>): Promise<void> {
    try {
      await fetch(`${this.baseUrl()}/api/v1/integrations/streamdeck/trigger`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(action),
      });
    } catch (err) {
      this.log("warn", `Failed sending StageLock action: ${String(err)}`);
    }
  }

  private connectWebSocket(): void {
    this.updateStatus(InstanceStatus.Connecting);
    const url = `ws://${this.config.host}:${this.config.port}/ws/telemetry`;
    const ws = new WebSocket(url);
    this.ws = ws;

    ws.on("open", () => {
      this.updateStatus(InstanceStatus.Ok);
      this.log("info", `Connected to StageLock at ${url}`);
    });

    ws.on("message", (raw) => {
      try {
        const msg = JSON.parse(raw.toString()) as { type?: string; cameras?: TelemetryCam[] };
        if (msg.type === "telemetry_batch" && Array.isArray(msg.cameras)) {
          this.cameras = msg.cameras;
          this.updateVariables();
          this.checkFeedbacks("tracking_state", "tally_state", "framing_preset", "ndi_active");
        }
      } catch {
        // ignore malformed telemetry
      }
    });

    ws.on("close", () => {
      this.updateStatus(InstanceStatus.Disconnected);
      this.reconnectTimer = setTimeout(() => this.connectWebSocket(), 2000);
    });

    ws.on("error", () => {
      ws.close();
    });
  }

  private buildVariables(): CompanionVariableDefinitions<VariablesSchema> {
    const vars: CompanionVariableDefinitions<VariablesSchema> = {};
    for (let i = 1; i <= 4; i++) {
      vars[`cam_${i}_track`] = { name: `Camera ${i} Tracking State` };
      vars[`cam_${i}_preset`] = { name: `Camera ${i} Framing Preset` };
      vars[`cam_${i}_focus_pct`] = { name: `Camera ${i} Focus Sharpness %` };
      vars[`cam_${i}_ndi`] = { name: `Camera ${i} NDI Output Active` };
    }
    return vars;
  }

  private updateVariables(): void {
    const values: Partial<VariablesSchema> = {};
    this.cameras.slice(0, 4).forEach((cam, idx) => {
      const n = idx + 1;
      values[`cam_${n}_track`] = cam.tracking_state.toUpperCase();
      values[`cam_${n}_preset`] = cam.framing_preset;
      values[`cam_${n}_focus_pct`] = Math.round((cam.focus_sharpness ?? 0) * 100);
      values[`cam_${n}_ndi`] = cam.ndi_output_active ? "LIVE" : "OFF";
    });
    this.setVariableValues(values);
  }

  private buildActions(): CompanionActionDefinitions<ActionsSchema> {
    const camField = {
      type: "number" as const,
      id: "camera_index" as const,
      label: "Camera Index (1-based)",
      default: 1,
      min: 1,
      max: 16,
    };

    return {
      toggle_track: {
        name: "Auto-Track: Toggle ON/OFF",
        options: [camField],
        callback: async (ev) => {
          await this.postSurfaceAction({
            type: "toggle_auto_track",
            camera_index: Math.max(0, Number(ev.options.camera_index) - 1),
          });
        },
      },
      set_framing_preset: {
        name: "Framing: Set Shot Preset",
        options: [
          camField,
          {
            type: "dropdown",
            id: "preset",
            label: "Shot Preset",
            default: "waist_up",
            choices: [
              { id: "close_up", label: "Close-Up" },
              { id: "waist_up", label: "Waist Up" },
              { id: "full_height", label: "Full Height" },
            ],
          },
        ],
        callback: async (ev) => {
          await this.postSurfaceAction({
            type: "set_framing_preset",
            camera_index: Math.max(0, Number(ev.options.camera_index) - 1),
            preset: String(ev.options.preset),
          });
        },
      },
      set_thirds: {
        name: "Framing: Set Rule of Thirds",
        options: [
          camField,
          {
            type: "dropdown",
            id: "thirds",
            label: "Composition",
            default: "center",
            choices: [
              { id: "left_third", label: "Left Third (1/3)" },
              { id: "center", label: "Center" },
              { id: "right_third", label: "Right Third (2/3)" },
              { id: "auto_lead_room", label: "Auto Lead-Room" },
            ],
          },
        ],
        callback: async (ev) => {
          await this.postSurfaceAction({
            type: "set_rule_of_thirds",
            camera_index: Math.max(0, Number(ev.options.camera_index) - 1),
            thirds: String(ev.options.thirds),
          });
        },
      },
      one_push_af: {
        name: "Focus: Trigger One-Push Auto-Focus",
        options: [camField],
        callback: async (ev) => {
          await this.postSurfaceAction({
            type: "trigger_one_push_af",
            camera_index: Math.max(0, Number(ev.options.camera_index) - 1),
          });
        },
      },
      recall_preset: {
        name: "PTZ: Recall Camera Preset",
        options: [
          camField,
          { type: "number", id: "slot", label: "Preset Slot", default: 1, min: 1, max: 127 },
        ],
        callback: async (ev) => {
          await this.postSurfaceAction({
            type: "recall_preset",
            camera_index: Math.max(0, Number(ev.options.camera_index) - 1),
            slot: Number(ev.options.slot),
          });
        },
      },
      toggle_ndi: {
        name: "NDI® Output: Toggle Re-Broadcast",
        options: [camField],
        callback: async (ev) => {
          await this.postSurfaceAction({
            type: "toggle_ndi_out",
            camera_index: Math.max(0, Number(ev.options.camera_index) - 1),
          });
        },
      },
      dial_zoom: {
        name: "Stream Deck + Dial: Optical Zoom Step",
        options: [
          camField,
          { type: "number", id: "delta", label: "Step (-1.0 Wide .. +1.0 Tele)", default: 0.25, min: -1, max: 1 },
        ],
        callback: async (ev) => {
          await this.postSurfaceAction({
            type: "dial_zoom_step",
            camera_index: Math.max(0, Number(ev.options.camera_index) - 1),
            delta: Number(ev.options.delta),
          });
        },
      },
      dial_exposure: {
        name: "Stream Deck + Dial: Exposure Step",
        options: [
          camField,
          { type: "number", id: "delta", label: "EV Step", default: 0.1, min: -1, max: 1 },
        ],
        callback: async (ev) => {
          await this.postSurfaceAction({
            type: "dial_exposure_step",
            camera_index: Math.max(0, Number(ev.options.camera_index) - 1),
            delta: Number(ev.options.delta),
          });
        },
      },
      vmix_cut: {
        name: "vMix: Cut to Input",
        options: [{ type: "textinput", id: "input", label: "Input Number / Name / GUID", default: "1" }],
        callback: async (ev) => {
          await this.postSurfaceAction({ type: "vmix_cut", input: String(ev.options.input) });
        },
      },
      vmix_fade: {
        name: "vMix: Fade to Input",
        options: [
          { type: "textinput", id: "input", label: "Input Number / Name / GUID", default: "1" },
          { type: "number", id: "duration_ms", label: "Duration (ms)", default: 500, min: 50, max: 5000 },
        ],
        callback: async (ev) => {
          await this.postSurfaceAction({
            type: "vmix_fade",
            input: String(ev.options.input),
            duration_ms: Number(ev.options.duration_ms),
          });
        },
      },
    };
  }

  private buildFeedbacks(): CompanionFeedbackDefinitions<FeedbacksSchema> {
    const camField = {
      type: "number" as const,
      id: "camera_index" as const,
      label: "Camera Index (1-based)",
      default: 1,
      min: 1,
      max: 16,
    };

    return {
      tracking_state: {
        type: "boolean",
        name: "Camera Auto-Track Locked",
        defaultStyle: { bgcolor: combineRgb(22, 163, 74), color: combineRgb(255, 255, 255) },
        options: [camField],
        callback: (fb) => {
          const cam = this.cameras[Math.max(0, Number(fb.options.camera_index) - 1)];
          return Boolean(cam?.auto_track_enabled && cam?.tracking_state === "locked");
        },
      },
      tally_state: {
        type: "boolean",
        name: "Camera Live on vMix Program",
        defaultStyle: { bgcolor: combineRgb(220, 38, 38), color: combineRgb(255, 255, 255) },
        options: [camField],
        callback: (fb) => {
          const cam = this.cameras[Math.max(0, Number(fb.options.camera_index) - 1)];
          return cam?.tally === "program";
        },
      },
      framing_preset: {
        type: "boolean",
        name: "Active Framing Shot Preset",
        defaultStyle: { bgcolor: combineRgb(2, 132, 199), color: combineRgb(255, 255, 255) },
        options: [
          camField,
          {
            type: "dropdown",
            id: "preset",
            label: "Preset",
            default: "waist_up",
            choices: [
              { id: "close_up", label: "Close-Up" },
              { id: "waist_up", label: "Waist Up" },
              { id: "full_height", label: "Full Height" },
            ],
          },
        ],
        callback: (fb) => {
          const cam = this.cameras[Math.max(0, Number(fb.options.camera_index) - 1)];
          return cam?.framing_preset === fb.options.preset;
        },
      },
      ndi_active: {
        type: "boolean",
        name: "Camera NDI® Output Active",
        defaultStyle: { bgcolor: combineRgb(14, 165, 233), color: combineRgb(255, 255, 255) },
        options: [camField],
        callback: (fb) => {
          const cam = this.cameras[Math.max(0, Number(fb.options.camera_index) - 1)];
          return Boolean(cam?.ndi_output_active);
        },
      },
    };
  }

  private buildPresetSections(): CompanionPresetSection<ModuleSchema>[] {
    return [
      {
        id: "cam1_auto_track",
        name: "Camera 1 Auto-Track",
        definitions: ["cam1_track_toggle"],
      },
      {
        id: "cam1_framing",
        name: "Camera 1 Framing",
        definitions: ["cam1_closeup", "cam1_waistup", "cam1_full"],
      },
    ];
  }

  private buildPresets(): CompanionPresetDefinitions<ModuleSchema> {
    const trackingActiveStyle = { bgcolor: combineRgb(22, 163, 74), color: combineRgb(255, 255, 255) };
    const framingActiveStyle = { bgcolor: combineRgb(2, 132, 199), color: combineRgb(255, 255, 255) };

    return {
      cam1_track_toggle: {
        type: "simple",
        name: "Toggle Auto-Track",
        style: { text: "CAM 1\\nTRACK", size: "14", color: combineRgb(255, 255, 255), bgcolor: combineRgb(30, 41, 59) },
        steps: [{ down: [{ actionId: "toggle_track", options: { camera_index: 1 } }], up: [] }],
        feedbacks: [{ feedbackId: "tracking_state", options: { camera_index: 1 }, style: trackingActiveStyle }],
      },
      cam1_closeup: {
        type: "simple",
        name: "Close-Up",
        style: { text: "CLOSE\\nUP", size: "14", color: combineRgb(255, 255, 255), bgcolor: combineRgb(30, 41, 59) },
        steps: [
          { down: [{ actionId: "set_framing_preset", options: { camera_index: 1, preset: "close_up" } }], up: [] },
        ],
        feedbacks: [
          { feedbackId: "framing_preset", options: { camera_index: 1, preset: "close_up" }, style: framingActiveStyle },
        ],
      },
      cam1_waistup: {
        type: "simple",
        name: "Waist Up",
        style: { text: "WAIST\\nUP", size: "14", color: combineRgb(255, 255, 255), bgcolor: combineRgb(30, 41, 59) },
        steps: [
          { down: [{ actionId: "set_framing_preset", options: { camera_index: 1, preset: "waist_up" } }], up: [] },
        ],
        feedbacks: [
          { feedbackId: "framing_preset", options: { camera_index: 1, preset: "waist_up" }, style: framingActiveStyle },
        ],
      },
      cam1_full: {
        type: "simple",
        name: "Full Height",
        style: { text: "FULL\\nHEIGHT", size: "14", color: combineRgb(255, 255, 255), bgcolor: combineRgb(30, 41, 59) },
        steps: [
          { down: [{ actionId: "set_framing_preset", options: { camera_index: 1, preset: "full_height" } }], up: [] },
        ],
        feedbacks: [
          {
            feedbackId: "framing_preset",
            options: { camera_index: 1, preset: "full_height" },
            style: framingActiveStyle,
          },
        ],
      },
    };
  }
}
