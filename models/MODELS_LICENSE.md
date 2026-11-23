# Permissive Open-Source Vision Model Licenses

VideoController strictly uses **permissive (Apache-2.0 / MIT)** computer vision weights so the entire project remains MIT / Apache-2.0 friendly (avoiding AGPL-3.0 model weights).

| Role | Filename | Upstream Model | License |
| :--- | :--- | :--- | :--- |
| Person Detector | `rtmdet-s.onnx` | OpenMMLab MMDetection RTMDet-s | Apache-2.0 |
| 17-Keypoint Pose | `rtmpose-s.onnx` | OpenMMLab MMPose RTMPose-s | Apache-2.0 |
| Body Re-ID | `osnet_x0_25.onnx` | Torchreid OSNet x0.25 | MIT |

Run `.\scripts\download-models.ps1` to download the ONNX weights into this folder.
