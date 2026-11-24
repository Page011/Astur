--!strict
-- The current media session: a play or pause icon (what the player is
-- doing now) and the track. A click plays or pauses, a right click skips to
-- the next track. `hide` is "idle" (hidden while there is no session) or
-- "never". Format fields: {artist} {title} {album}. Classes: playing,
-- paused, idle, stale.
local rsb = require("@rsb")
local text = require("./lib/format")

return rsb.widget {
	options = {
		format = "{artist} - {title}",
		max_width = 280,
		hide = "idle",
	},
	permissions = { control = { "media" }, read = { "media" } },
	setup = function(props, ctx)
		local media = rsb.services.media

		local function glyph(): string
			if media.stale or media.available ~= true then
				return "music"
			end
			return if media.playing then "play" else "pause"
		end

		return rsb.row {
			-- A paused track still shows, so it can be resumed.
			visible = function(): boolean
				if media.stale then
					return true
				end
				return media.available == true or props.hide == "never"
			end,
			class = function(): string
				if media.stale then
					return "item media button stale"
				elseif media.available ~= true then
					return "item media button idle"
				end
				return if media.playing then "item media button playing" else "item media button paused"
			end,
			rsb.icon { name = glyph :: any },
			rsb.text {
				class = "value ellipsis",
				style = { max_width = props.max_width },
				text = function(): string
					if media.stale then
						return "--"
					elseif media.available ~= true then
						return ""
					end
					return text.tidy(text.apply(props.format, {
						artist = media.artist,
						title = media.title,
						album = media.album,
					}))
				end,
			},
			on_click = function()
				media:play_pause()
			end,
			on_right_click = function()
				media:next()
			end,
		}
	end,
}
