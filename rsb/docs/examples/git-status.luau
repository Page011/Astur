--!strict
-- How many files a git repository has changed, from `git status --short`.
--
-- Save as widgets\git-status.luau, then in bar.toml:
--
--   [[bar]]
--   right = ["git-status", "clock"]
--
--   [widget.git-status]
--   repo = "C:/work/project"
--   every = "30s"
--
-- The widget asks for the right to run git (and only git). RSB then shows
-- it in the grant line of the log: exec:git. Git can run other programs
-- through its options, so the line marks it "can run any program".
local rsb = require("@rsb")

return rsb.widget {
	options = {
		repo = "C:/work/project",
		every = "30s",
	},
	permissions = { exec = { "git" } },
	setup = function(props, ctx)
		-- nil until the first answer; false when git failed.
		local changed = rsb.poll(props.every, function(): number | boolean
			local result = rsb.exec.run({ "git", "-C", props.repo, "status", "--short" }, { timeout = "10s" })
			if not result.ok then
				return false
			end
			local count = 0
			for _ in string.gmatch(result.stdout, "[^\r\n]+") do
				count += 1
			end
			return count
		end)

		return rsb.row {
			class = function(): string
				return if changed.value == false then "item git stale" else "item git"
			end,
			rsb.icon { name = "info" },
			rsb.text {
				class = "value",
				text = function(): string
					local c = changed.value
					if c == nil or c == false then
						return "--"
					end
					return if c == 0 then "clean" else `{c} changed`
				end,
			},
		}
	end,
}
