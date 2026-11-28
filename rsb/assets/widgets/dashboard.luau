--!strict
-- The button in the middle of the bar and the panel that grows out of it:
-- quick tiles (power mode, Bluetooth, audio output, apps, record, power menu)
-- and a page for each. A click on the button opens or closes the popup called
-- "dashboard" on this monitor; `rsb popup launcher` opens the same panel on
-- the apps page with the search focused. Escape goes back to the tiles from
-- a page (the tile it came from takes the focus), and closes the panel from
-- the tiles and from the apps page.
-- The record tile does what the record widget does: it opens Windows' screen
-- recorder, or runs `record_command` (a list such as ["obs64",
-- "--startrecording"], or a string split by the Windows argument rules) when
-- bar.toml sets one under [widget.dashboard].
-- Class: item, dashboard, button (the button); panel, dashboard (the panel).
local rsb = require("@rsb")
local panel = require("./lib/panel")
local cmdline = require("./lib/cmdline")

local RECORDER = "ms-screenclip://capture/video?enabledModes=RecordAllModes&user-agent=rsb"
local ROWS_PER_PAGE = 8
local MODES = {
	{ mode = "efficiency", label = "Best power efficiency", icon = "leaf" },
	{ mode = "balanced", label = "Balanced", icon = "balance" },
	{ mode = "performance", label = "Best performance", icon = "rocket" },
}

-- A program and its arguments from either form of the option.
local function argv(value: any): { string }?
	if type(value) == "string" then
		local words = cmdline.split(value)
		return if #words > 0 then words else nil
	elseif type(value) == "table" and #value > 0 then
		return value
	end
	return nil
end

return rsb.widget {
	about = { title = "Dashboard", opens = { "dashboard", "launcher", "power" } },
	options = {
		record_command = (nil :: any),
	},
	optional = { "record_command" },
	permissions = {
		control = { "power", "bluetooth", "audio", "apps" },
		read = { "bluetooth", "apps" },
		exec = { "$option:record_command" },
	},
	setup = function(props, ctx)
		local record = argv(props.record_command)
		local shown = rsb.state(false)
		local page = rsb.state("home")
		-- Set by the apps page while it exists: it takes the keys the list needs.
		local page_key: { fn: ((any) -> ())? } = { fn = nil }

		-- The page the panel came back from: its tile takes the focus.
		local origin: string? = nil

		local function home()
			local from = page.value :: string
			origin = if from ~= "home" then from else nil
			page.value = "home"
		end

		-- ------------------------------------------------------------ pieces

		-- A header card: the icon (it goes back to the tiles), then what the page adds.
		-- `focus_back` gives the back button the keyboard focus on entry.
		local function header(icon: string?, middle: any?, trailing: any?, focus_back: boolean?): any
			local card: any = { class = "card head" }
			if icon ~= nil then
				table.insert(card, panel.icon_button(icon, "Back", home, "icon-btn head-icon", focus_back))
			end
			if middle ~= nil then
				table.insert(card, middle)
			end
			if trailing ~= nil then
				table.insert(card, trailing)
			end
			return rsb.row(card)
		end

		-- ------------------------------------------------------------ home

		local function tile(icon: string, label: string, on_click: () -> (), target: string?): any
			return rsb.column {
				class = "tile",
				focusable = true,
				autofocus = target ~= nil and origin == target,
				tooltip = label,
				rsb.icon { svg = icon },
				on_click = on_click,
			}
		end

		local function tiles(): any
			return rsb.column {
				class = "tiles",
				key = "tiles",
				nav = "grid",
				rsb.row {
					class = "tile-row",
					tile("bolt", "Power mode", function()
						page.value = "power"
					end, "power"),
					tile("bluetooth", "Bluetooth", function()
						page.value = "bluetooth"
					end, "bluetooth"),
					tile("speaker", "Audio output", function()
						page.value = "audio"
					end, "audio"),
				},
				rsb.row {
					class = "tile-row",
					tile("apps", "Apps", function()
						page.value = "apps"
					end, "apps"),
					tile("video", "Record the screen", function()
						shown.value = false
						if record ~= nil then
							rsb.exec.run(record, { timeout = "20s", max_output = 4096 })
						else
							rsb.services.apps:open_uri(RECORDER)
						end
					end),
					tile("power", "Power menu", function()
						rsb.popups.toggle("power")
						shown.value = false
					end),
				},
			}
		end

		-- ------------------------------------------------------------ power mode

		local function power_page(): any
			local power = rsb.services.power
			local function usable(): boolean
				return power.mode_available == true and not power.stale
			end
			local rows = rsb.each(MODES, function(m: any): any
				return rsb.row {
					class = function(): string
						if power.mode == m.mode and not power.stale then
							return "row mode current"
						end
						return "row mode"
					end,
					focusable = true,
					disabled = function(): boolean
						return not usable()
					end,
					rsb.icon { svg = m.icon },
					rsb.text { class = "name", text = m.label },
					on_click = function()
						if usable() then
							power:set_mode(m.mode)
						end
					end,
				}
			end, function(m: any): string
				return m.mode
			end)
			return rsb.column {
				class = "page",
				key = "page-power",
				header("bolt", nil, nil, true),
				rsb.scroll {
					class = "card list",
					nav = "vertical",
					function(): { any }
						local why = panel.service_note(power, "Power mode")
						if why == nil and power.mode_available ~= true then
							why = "Power mode is not available on this PC"
						end
						if why ~= nil then
							return { panel.note(why) }
						end
						return rows()
					end,
				},
			}
		end

		-- ------------------------------------------------------------ Bluetooth

		local function bluetooth_page(): any
			local bt = rsb.services.bluetooth
			local query = rsb.state("")

			local function visible(): { any }
				local list = bt.devices :: { any }?
				if bt.stale or list == nil then
					return {}
				end
				local q = query.value
				if q == "" then
					return list
				end
				return rsb.fuzzy(q, list, {
					key = function(d: any): string
						return d.name
					end,
				})
			end

			local rows = rsb.each(visible, function(d: any): any
				local id = d.id :: string
				local actions: any = { class = "actions" }
				if d.can_connect then
					table.insert(
						actions,
						panel.icon_button(
							if d.connected then "bluetooth-off" else "bluetooth",
							if d.connected then "Disconnect" else "Connect",
							function()
								if d.connected then
									bt:disconnect(id)
								else
									bt:connect(id)
								end
							end
						)
					)
				end
				-- Forgetting a pairing cannot be undone from here: the first press arms
				-- the button for three seconds, the second one forgets.
				local armed = rsb.state(false)
				table.insert(
					actions,
					rsb.row {
						class = function(): string
							return if armed.value then "icon-btn forget armed" else "icon-btn forget"
						end,
						focusable = true,
						tooltip = "Forget this device",
						rsb.icon {
							svg = "trash",
							visible = function(): boolean
								return not armed.value
							end,
						},
						rsb.text {
							class = "confirm",
							text = "FORGET?",
							visible = function(): boolean
								return armed.value
							end,
						},
						on_click = function()
							if armed.value then
								armed.value = false
								bt:forget(id)
							else
								armed.value = true
								rsb.timer.after(3, function()
									armed.value = false
								end)
							end
						end,
					}
				)
				local line: any = {
					class = if d.connected then "row device connected" else "row device",
					rsb.text { class = "name ellipsis", text = d.name },
				}
				if d.battery ~= nil then
					table.insert(line, rsb.text { class = "level", text = string.format("%d%%", math.floor(d.battery + 0.5)) })
				end
				table.insert(line, rsb.row(actions))
				return rsb.row(line)
			end, function(d: any): string
				return d.id
			end)

			local function body(): { any }
				local why = panel.service_note(bt, "Bluetooth")
				if why ~= nil then
					return { panel.note(why) }
				end
				if bt.available ~= true then
					return { panel.note("No Bluetooth adapter") }
				end
				if bt.enabled ~= true then
					local off: any = { class = "off", panel.note("Bluetooth is off") }
					if bt.can_toggle then
						table.insert(
							off,
							rsb.row {
								class = "row action",
								focusable = true,
								rsb.text { text = "Turn on" },
								on_click = function()
									bt:set_enabled(true)
								end,
							}
						)
					end
					return { rsb.column(off) }
				end
				if #visible() == 0 then
					return { panel.note(if query.value == "" then "No paired devices" else "No devices match") }
				end
				return rows()
			end

			return rsb.column {
				class = "page bluetooth",
				key = "page-bluetooth",
				header(
					"bluetooth",
					rsb.input {
						class = "search",
						value = query,
						autofocus = true,
						placeholder = "Search for a device...",
					},
					panel.icon_button("refresh", "Scan for devices", function()
						bt:rescan()
					end)
				),
				rsb.scroll { class = "card list", nav = "vertical", body },
			}
		end

		-- ------------------------------------------------------------ audio

		local function audio_page(): any
			local audio = rsb.services.audio
			local rows = panel.device_rows(audio)
			return rsb.column {
				class = "page",
				key = "page-audio",
				header("volume", nil, nil, true),
				rsb.scroll {
					class = "card list",
					function(): { any }
						local why = panel.service_note(audio, "Audio")
						if why ~= nil then
							return { panel.note(why) }
						end
						if audio.available ~= true or #((audio.devices :: { any }?) or {}) == 0 then
							return { panel.note("No output devices") }
						end
						return rows()
					end,
				},
			}
		end

		-- ------------------------------------------------------------ apps (the launcher)

		local function apps_page(): any
			local apps = rsb.services.apps
			local query = rsb.state("")

			local items = rsb.computed(function(): { any }
				if apps.stale then
					return {}
				end
				local all = (apps.apps :: { any }?) or {}
				local q = query.value
				if q == "" then
					return all
				end
				return rsb.fuzzy(q, all, {
					key = function(a: any): string
						return a.name
					end,
					limit = 50,
				})
			end)

			-- Which row the keyboard is on. Each row has a mark (a state: 0 = not selected,
			-- otherwise a count that grows each time the row is selected, so the list
			-- scrolls to it again). Moving the selection changes two marks, never all of them.
			local marks: { [string]: any } = {}
			local current = ""
			local count = 0

			local function select_id(id: string)
				count += 1
				local old = marks[current]
				if old ~= nil and current ~= id then
					old.value = 0
				end
				current = id
				local m = marks[id]
				if m ~= nil then
					m.value = count
				end
			end

			-- The selection is row 1 whenever the query changes, and stays on its row
			-- while that row is listed (the list also rebuilds when the apps change).
			local last_query = ""
			local function settle_selection(list: { any })
				local fresh = query.value ~= last_query
				last_query = query.value
				if not fresh then
					for i = 1, #list do
						if list[i].id == current then
							return
						end
					end
				end
				count += 1
				current = if #list > 0 then list[1].id else ""
				for id, m in marks do
					m.value = if id == current then count else 0
				end
			end

			local function index_of(id: string): number
				local list = items:peek()
				for i = 1, #list do
					if list[i].id == id then
						return i
					end
				end
				return 1
			end

			local function select_at(i: number)
				local list = items:peek()
				if #list > 0 then
					select_id(list[math.clamp(i, 1, #list)].id)
				end
			end

			local function launch(id: string)
				apps:launch(id)
				shown.value = false
			end

			local function on_key(e: any)
				local key = e.key :: string
				if key == "ArrowDown" then
					select_at(index_of(current) + 1)
				elseif key == "ArrowUp" then
					select_at(index_of(current) - 1)
				elseif key == "PageDown" then
					select_at(index_of(current) + ROWS_PER_PAGE)
				elseif key == "PageUp" then
					select_at(index_of(current) - ROWS_PER_PAGE)
				elseif key == "Home" then
					select_at(1)
				elseif key == "End" then
					select_at(#items:peek())
				elseif key == "Enter" then
					if current ~= "" then
						launch(current)
					end
				end
			end
			page_key.fn = on_key
			rsb.on_cleanup(function()
				if page_key.fn == on_key then
					page_key.fn = nil
				end
			end)

			local rows = rsb.each(items, function(app: any): any
				local id = app.id :: string
				local mark = rsb.state(if id == current then count else 0)
				marks[id] = mark
				rsb.on_cleanup(function()
					if marks[id] == mark then
						marks[id] = nil
					end
				end)
				return rsb.row {
					class = function(): string
						return if mark.value > 0 then "row app selected" else "row app"
					end,
					reveal = function(): any
						return if mark.value > 0 then mark.value else false
					end,
					rsb.text { text = app.name },
					on_click = function()
						launch(id)
					end,
				}
			end, function(app: any): string
				return app.id
			end)

			return rsb.column {
				class = "page",
				key = "page-apps",
				header(
					nil,
					rsb.input {
						class = "search",
						value = query,
						autofocus = true,
						placeholder = "Search apps...",
					},
					nil
				),
				rsb.scroll {
					class = "card list",
					function(): { any }
						local why = panel.service_note(apps, "The app list", "the app list")
						if why ~= nil then
							return { panel.note(why) }
						end
						local list = items.value
						if #list == 0 then
							return { panel.note(if query.value == "" then "No apps" else "No apps match") }
						end
						settle_selection(list)
						return rows()
					end,
				},
			}
		end

		-- ------------------------------------------------------------ the popup

		local function current(): { any }
			local p = page.value
			if p == "power" then
				return { power_page() }
			elseif p == "bluetooth" then
				return { bluetooth_page() }
			elseif p == "audio" then
				return { audio_page() }
			elseif p == "apps" then
				return { apps_page() }
			end
			return { tiles() }
		end

		return rsb.row {
			class = "item dashboard button",
			rsb.icon { svg = "logo" },
			on_click = function()
				rsb.popups.toggle("dashboard")
			end,
			rsb.popup {
				open = shown,
				name = { "dashboard", "launcher" },
				class = "panel dashboard",
				placement = "below",
				attach = true,
				dismiss = { escape = false },
				on_open = function(name: string)
					origin = nil
					page.value = if name == "launcher" then "apps" else "home"
				end,
				on_key = function(e: any)
					if e.key == "Escape" then
						local p = page.value
						if p == "home" or p == "apps" then
							shown.value = false
						else
							home()
						end
						return
					end
					local handler = page_key.fn
					if handler ~= nil then
						handler(e)
					end
				end,
				function(): { any }
					return { rsb.column { class = "panel-body", current } }
				end,
			},
		}
	end,
}
