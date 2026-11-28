--!strict
-- Shows a number that another program keeps in a text file.
--
-- Save as widgets\counter.luau, then in bar.toml:
--
--   [[bar]]
--   right = ["counter", "clock"]
--
-- The file is watched: write a new number to it and the bar follows within
-- a moment. A missing file shows --. A widget may read only its own folder
-- plus the files its `file` permission lists, so the path appears twice:
-- change both lines to watch another file.
local rsb = require("@rsb")

local PATH = "~/count.txt"

return rsb.widget {
	permissions = { file = { "~/count.txt" } },
	setup = function(props, ctx)
		local counter = rsb.file.watch(PATH)

		return rsb.row {
			class = function(): string
				return if counter.value == nil then "item counter stale" else "item counter"
			end,
			rsb.icon { name = "bell" },
			rsb.text {
				class = "value",
				text = function(): string
					local raw = counter.value
					if raw == nil then
						return "--"
					end
					local n = tonumber(raw)
					return if n ~= nil then tostring(n) else "--"
				end,
			},
		}
	end,
}
