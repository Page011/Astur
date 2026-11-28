--!strict
-- Microphone and Bluetooth at a glance: a mic icon, a Bluetooth icon and the
-- battery of the first connected Bluetooth device that reports one. Both
-- icons are buttons: the mic toggles the mute of the default recording
-- device, the Bluetooth icon turns the radio on or off. A muted mic and a
-- radio that is off are drawn dim (classes `off`), a mic an app is recording
-- from has the class `live`. The segment is a narrow sliver until the
-- pointer is over its group (the theme decides how it opens).
--   mic        false leaves the microphone out
--   bluetooth  false leaves Bluetooth and the device battery out
-- Classes of the segment: item, status. Inside: status-content, with
-- status-btn (mic, bt) and status-level.
local rsb = require("@rsb")

return rsb.widget {
	about = { title = "Mic and Bluetooth" },
	options = {
		mic = true,
		bluetooth = true,
	},
	permissions = { control = { "bluetooth", "microphone" }, read = { "bluetooth" } },
	setup = function(props, ctx)
		local mic = rsb.services.microphone
		local bt = rsb.services.bluetooth

		local function mic_off(): boolean
			return not mic.stale and mic.muted == true
		end

		local function bt_off(): boolean
			return not bt.stale and bt.enabled == false
		end

		-- The battery of the first connected device that reports one.
		local function device_battery(): number?
			if bt.stale then
				return nil
			end
			for _, d in (bt.devices :: { any }?) or {} do
				if d.connected and d.battery ~= nil then
					return d.battery :: number
				end
			end
			return nil
		end

		local function connected(): boolean
			for _, d in (bt.devices :: { any }?) or {} do
				if d.connected then
					return true
				end
			end
			return false
		end

		local parts: { any } = {}
		if props.mic then
			table.insert(
				parts,
				rsb.row {
					class = function(): string
						if mic.stale then
							return "status-btn mic button stale"
						elseif mic_off() then
							return "status-btn mic button off"
						end
						return if mic.in_use == true then "status-btn mic button live" else "status-btn mic button"
					end,
					visible = function(): boolean
						return mic.stale or mic.available == true
					end,
					tooltip = function(): string
						return if mic_off() then "Microphone muted" else "Microphone on"
					end,
					rsb.icon {
						svg = function(): string
							return if mic_off() then "mic-off" else "mic"
						end,
					},
					on_click = function()
						mic:toggle_mute()
					end,
				}
			)
		end
		if props.bluetooth then
			table.insert(
				parts,
				rsb.row {
					class = function(): string
						if bt.stale then
							return "status-btn bt button stale"
						end
						return if bt_off() then "status-btn bt button off" else "status-btn bt button"
					end,
					visible = function(): boolean
						return bt.stale or bt.available == true
					end,
					tooltip = function(): string
						return if bt_off() then "Bluetooth off" else "Bluetooth on"
					end,
					rsb.icon {
						svg = function(): string
							if bt_off() then
								return "bluetooth-off"
							end
							return if connected() then "bluetooth-connected" else "bluetooth"
						end,
					},
					on_click = function()
						local enabled = bt.enabled :: boolean?
						if enabled ~= nil then
							bt:set_enabled(not enabled)
						end
					end,
				}
			)
			table.insert(
				parts,
				rsb.text {
					class = function(): string
						return if bt_off() then "value status-level off" else "value status-level"
					end,
					visible = function(): boolean
						return device_battery() ~= nil
					end,
					text = function(): string
						local level = device_battery()
						return if level == nil then "" else string.format("%d%%", math.floor(level + 0.5))
					end,
				}
			)
		end

		return rsb.row {
			class = "item status",
			rsb.row {
				class = "status-content",
				function(): { any }
					return parts
				end,
			},
		}
	end,
}
