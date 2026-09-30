# YASB workspaces

Astur can serve the workspace part of GlazeWM's localhost WebSocket protocol.
Use YASB's existing `glazewm.workspaces.GlazewmWorkspacesWidget`; no YASB fork,
GlazeWM process, helper script, or custom plugin is required.

## Setup

In **Astur Settings > YASB integration**, enable the integration, choose the port,
and click **Save**. The same page can copy the server address, widget YAML with
your selected port, and example CSS. Follow its YASB setup steps, then turn off
**Show Astur's built-in bar** and save. To restore the Astur bar, turn it back on.

The settings exe embeds the examples, so these copy buttons work with a portable
installation too. For manual configuration:

1. Run an Astur build containing this integration. In
   `%USERPROFILE%\.astur\astur.conf`, add:

   ```ini
   yasb_enabled = true
   yasb_port = 6123
   ```

2. Disable Astur's built-in bar in `%USERPROFILE%\.astur\navbar.conf`:

   ```ini
   enabled = false
   ```

   Both files hot-reload. `ipc_enabled` is separate and can stay false.

3. Merge the widget definition in [workspaces.yaml](workspaces.yaml) into your
   YASB configuration. Add `astur_workspaces` to your bar's `widgets.left` list.
   In that bar's `window_flags`, set `windows_app_bar: true`. This reserves space;
   Astur responds to Windows work-area changes and tiles below/above the bar.
   Existing GlazeWM workspace CSS works; the example CSS supplies a basic style.

4. Restart/reload YASB. Use `ws://127.0.0.1:6123` explicitly. If another app owns
   port 6123, choose a free port in **both** Astur and the widget configuration.
   Stop GlazeWM if it is running: two window managers must not manage the desktop.

## Supported behavior

- Live active, focused, occupied, and empty workspace indicators.
- Configured workspace labels and optional app icons, including floating-window
  filtering. YASB reads icons from the real window handles.
- Click to focus a workspace; wheel to cycle. With `monitor_exclusive: true`,
  wheel targets the monitor under the cursor. With `false`, wheel cycles globally.
- Shared workspace numbering and per-monitor workspaces. The latter use unique
  internal names (`m<monitor-handle>-w<number>`) while displaying friendly labels.
- Reconnect after restarting Astur, and hot enable/disable/port changes.

Only the workspace widget is supported. This is not a full GlazeWM API: binding
modes, tiling-direction controls, arbitrary commands, and window title/process
metadata are not exposed. Unsupported commands return a failure response.

The server binds **127.0.0.1 only**, rejects browser Origin headers, and caps
clients/messages/buffers. Like GlazeWM's local WebSocket model, native local
processes can connect without a token, read workspace/window handles, and switch
workspaces. This is separate from Astur's owner-only named pipe. Do not proxy the
port to other machines. Binding errors are recorded in Astur's log.

Protocol checked against [YASB's client](https://github.com/amnweb/yasb/blob/5520b42b769d0136929a33eab5460d19f4f3f405/src/core/widgets/services/glazewm/client.py).
Loopback protocol tests cover subscriptions, state updates, switching commands,
reconnects, incomplete handshakes, and browser rejection. Visual styling and
mixed-monitor behavior should also be checked in your installed YASB version.
