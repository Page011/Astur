--!strict
-- Output volume. The wheel changes it by `step` percent, a click toggles
-- mute. `muted_format` replaces `format` while muted ("MUTE" for a word).
-- popup = true makes a click open the mixer instead (a popup called
-- "volume": a slider for the output volume, a button that mutes it, and one
-- slider row for every output device, with a button to make it the default
-- output); the wheel still changes the volume.
-- Classes: muted, absent (no output device), stale.
local rsb = require("@rsb")
local text = require("./lib/format")
local panel = require("./lib/panel")

return rsb.widget {
	options = {
		format = "{value}%",
		muted_format = "",
		label = "",
		step = 2,
		popup = false,
	},
	permissions = { control = { "audio" } },
	setup = function(props, ctx)
		local audio = rsb.services.audio

		local function glyph(): string
			if audio.stale or audio.available ~= true or audio.muted then
				return "volume_mute"
			end
			local volume = (audio.volume :: number?) or 0
			if volume <= 0 then
				return "volume_0"
			elseif volume < 34 then
				return "volume_1"
			elseif volume < 67 then
				return "volume_2"
			end
			return "volume_3"
		end

		local lead = if props.label ~= ""
			then rsb.text { class = "label", text = props.label }
			else rsb.icon { name = glyph :: any }

		-- The mixer popup: the master slider on top, the devices under it.
		local open = rsb.state(false)
		local function mixer(): { any }
			local rows = panel.device_rows(audio)
			local function level(): number
				return if audio.stale then 0 else ((audio.volume :: number?) or 0)
			end
			return {
				rsb.column {
					class = "volume-body",
					panel.slider_row("row output master", level, function(value: number, done: boolean)
						audio:set_volume(value)
					end, function(): boolean
						return audio.stale or audio.available ~= true
					end, {
						panel.icon_button(function(): string
							if audio.muted == true then
								return "volume-mute"
							end
							return if level() < 34 then "volume-low" else "volume"
						end, "Mute or unmute", function()
							audio:toggle_mute()
						end, "icon-btn lead"),
						rsb.text { class = "name", text = "Volume" },
						rsb.text {
							class = "level",
							text = function(): string
								return if audio.stale then "--" else text.apply("{value}%", { value = text.round(level()) })
							end,
						},
					}),
					rsb.scroll {
						class = "card list",
						function(): { any }
							local why = panel.service_note(audio, "Audio")
							if why ~= nil then
								return { panel.note(why) }
							end
							if #((audio.devices :: { any }?) or {}) == 0 then
								return { panel.note("No output devices") }
							end
							return rows()
						end,
					},
				},
			}
		end
		local pop: any = nil
		if props.popup then
			pop = rsb.popup {
				open = open,
				name = "volume",
				class = "panel volume",
				placement = "below",
				align = "center",
				mixer,
			}
		end

		return rsb.row {
			-- Nothing to show before the first reading.
			visible = function(): boolean
				return audio.stale or (audio.available :: boolean?) ~= nil
			end,
			class = function(): string
				if audio.stale then
					return "item volume button stale"
				elseif audio.available ~= true then
					return "item volume button absent"
				elseif audio.muted then
					return "item volume button muted"
				end
				return "item volume button"
			end,
			lead,
			rsb.text {
				class = "value",
				text = function(): string
					if audio.stale then
						return "--"
					elseif audio.available ~= true then
						return ""
					end
					local volume = audio.volume :: number?
					if volume == nil then
						return "--"
					end
					local shown = if audio.muted and props.muted_format ~= "" then props.muted_format else props.format
					return text.apply(shown, { value = text.round(volume) })
				end,
			},
			pop,
			on_click = function()
				if props.popup then
					open.value = not open.value
				elseif audio.available then
					audio:toggle_mute()
				end
			end,
			on_wheel = function(e)
				if audio.available then
					audio:change_volume(e.delta * props.step)
				end
			end,
		}
	end,
}
