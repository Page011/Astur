--!strict
-- Output volume. The wheel changes it by `step` percent, a click toggles
-- mute. `muted_format` replaces `format` while muted ("MUTE" for a word).
-- Classes: muted, absent (no output device), stale.
local rsb = require("@rsb")
local text = require("./lib/format")

return rsb.widget {
	options = {
		format = "{value}%",
		muted_format = "",
		label = "",
		step = 2,
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
			on_click = function()
				if audio.available then
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
