# AI Vision Models (`models/`)

Directory for optional permissive **ONNX** computer vision weights used by [`stagelock-vision`](../crates/stagelock-vision/README.md) via ONNX Runtime (`ort`).

> Binary `.onnx` and `.engine` model weights are excluded from Git via `.gitignore` to keep the repository lightweight.

## Supported Models

| Model | File | Purpose | License |
| :--- | :--- | :--- | :--- |
| **RTMDet-s** | `rtmdet_s.onnx` | Real-time multi-person detection | Apache-2.0 |
| **RTMPose-s** | `rtmpose_s.onnx` | 17 COCO body/face anatomical keypoints for Rule-of-Thirds framing & Face-ROI focus | Apache-2.0 |
| **OSNet x0.25** | `osnet_x0_25.onnx` | 512-dim appearance Re-ID embedding (falls back automatically to 96-dim spatial HSV Re-ID if absent) | MIT |

## Downloading Models

Run the helper script from the repository root:

```pwsh
.\scripts\download-models.ps1
```

## Permissive Licensing Policy

StageLock strictly uses **Apache-2.0 / MIT** models only (no AGPL-licensed YOLO weights). See [`MODELS_LICENSE.md`](./MODELS_LICENSE.md) for upstream attributions and compliance details.
