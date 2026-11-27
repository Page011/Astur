--!strict
-- One pill per workspace from a running window manager (Astur or GlazeWM
-- through the GlazeWM WebSocket, or komorebi). A click focuses a workspace,
-- the wheel steps through them. With no window manager the widget draws
-- nothing at all.
--   monitor = "own"  the workspaces of the bar's own monitor (matched by
--                    handle, then device name, then position; when none
--                    matches, every monitor's are shown)
--   monitor = "all"  every monitor's workspaces
--   label           "display_name" or "name"
--   style           "pills" (a rounded plate per workspace) or "text" (plain
--                   labels; the root also gets the class `plain`)
--   active_format   how the focused workspace's label reads, {label} is the
--                   label: "-{label}-" gives -8- for workspace 8
-- Pill classes: ws, focused, visible, occupied, empty.
local rsb = require("@rsb")
local text = require("./lib/format")

type Pill = {
	name: string,
	label: string,
	monitor: number,
	focused: boolean,
	visible: boolean,
	has_windows: boolean,
}

return rsb.widget {
	about = { title = "Workspaces" },
	options = {
		monitor = "own",
		hide_empty = false,
		label = "display_name",
		wheel = true,
		style = "pills",
		active_format = "{label}",
	},
	permissions = { control = { "workspaces" } },
	setup = function(props, ctx)
		local workspaces = rsb.services.workspaces
		local own = ctx.monitor

		-- The monitor entries this bar shows.
		local function shown(): { any }
			local monitors = (workspaces.monitors :: { any }?) or {}
			if props.monitor == "all" then
				return monitors
			end
			for _, m in monitors do
				if m.handle ~= nil and m.handle == own.handle then
					return { m }
				end
			end
			for _, m in monitors do
				if m.device ~= nil and string.lower(m.device) == string.lower(own.device) then
					return { m }
				end
			end
			for _, m in monitors do
				if m.index == own.index then
					return { m }
				end
			end
			return monitors
		end

		local function pills(): { Pill }
			local list: { Pill } = {}
			for _, m in shown() do
				for _, w in m.workspaces do
					local idle = not w.has_windows and not w.focused and not w.visible
					if not (props.hide_empty and idle) then
						table.insert(list, {
							name = w.name,
							label = if props.label == "name" then w.name else w.display_name,
							monitor = m.index,
							focused = w.focused,
							visible = w.visible,
							has_windows = w.has_windows,
						})
					end
				end
			end
			return list
		end

		local function pill_class(p: Pill): string
			if p.focused then
				return "ws focused button"
			elseif p.visible then
				return "ws visible button"
			end
			return if p.has_windows then "ws occupied button" else "ws empty button"
		end

		local function step(delta: number)
			local monitor = if props.monitor == "all" then nil else own.index
			if delta > 0 then
				workspaces:focus_previous(monitor)
			else
				workspaces:focus_next(monitor)
			end
		end

		local items = rsb.each(pills, function(p: Pill)
			return rsb.box {
				class = pill_class(p),
				rsb.text {
					text = if p.focused then text.apply(props.active_format, { label = p.label }) else p.label,
				},
				on_click = function()
					workspaces:focus(p.name)
				end,
			}
		end, function(p: Pill): string
			return tostring(p.monitor) .. ":" .. p.name
		end)

		local plain = if props.style == "text" then " plain" else ""

		return rsb.row {
			-- No window manager (or none yet): no pills and no space.
			visible = function(): boolean
				return workspaces.stale or (workspaces.connected == true and #pills() > 0)
			end,
			class = function(): string
				return if workspaces.stale then "workspaces stale" .. plain else "workspaces" .. plain
			end,
			-- A stale connection shows -- instead of pills that may be out of date.
			function(): { any }
				if workspaces.stale then
					return { rsb.box { class = "ws empty", key = "stale", rsb.text { text = "--" } } }
				end
				return items()
			end,
			on_wheel = if props.wheel
				then function(e: { delta: number })
					step(e.delta)
				end
				else nil,
		}
	end,
}
