# NDI® Trademark & Runtime Attribution

NDI® is a registered trademark of Vizrt NDI AB.

- Learn more about NDI: https://ndi.video
- Download NDI Tools & Runtime: https://ndi.video/tools

## Runtime Loading Policy

StageLock does **not** redistribute NDI Tools or statically link proprietary NDI SDK binaries. Instead, StageLock dynamically loads the user-installed or app-local NDI® 6 runtime library (`Processing.NDI.Lib.x64.dll` on Windows, `libndi.dylib` on macOS, `libndi.so` on Linux) at runtime via `NDI_RUNTIME_DIR_V6` (or standard NDI runtime paths).

If you distribute a commercial product based on StageLock with NDI® support enabled, ensure your distribution complies with the current Vizrt NDI SDK license terms at https://ndi.video.
