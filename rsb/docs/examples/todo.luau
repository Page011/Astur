--!strict
-- A short to-do list in a popup: type a line and press Enter to add it, click
-- a line to tick it off, press Escape (or click outside) to close.
--
-- Save as widgets\todo.luau, then in bar.toml:
--
--   [[bar]]
--   right = ["todo", "clock"]
--
-- The popup is built only while it is open, and the list lives in the widget,
-- so it starts empty after RSB restarts. The panel, card, row, note and input
-- looks come from the preset's style.css; nothing here needs a permission.
local rsb = require("@rsb")

local MAX_ITEMS = 50

return rsb.widget {
	setup = function(props, ctx)
		local shown = rsb.state(false)
		local lines = rsb.state({} :: { string })
		local draft = rsb.state("")

		local function add(text: string)
			local line = string.gsub(text, "^%s+", "")
			line = string.gsub(line, "%s+$", "")
			if line == "" or #lines.value >= MAX_ITEMS then
				return
			end
			local next = table.clone(lines.value)
			table.insert(next, line)
			lines.value = next
			draft.value = ""
		end

		local function done(index: number)
			local next = table.clone(lines.value)
			table.remove(next, index)
			lines.value = next
		end

		local rows = rsb.each(function(): { string }
			return lines.value
		end, function(line: string, index: number): any
			return rsb.row {
				class = "row",
				focusable = true,
				tooltip = "Tick off",
				rsb.text { class = "name ellipsis", text = line },
				on_click = function()
					done(index)
				end,
			}
		end)

		return rsb.row {
			class = "item todo button",
			rsb.icon { svg = "check" },
			rsb.text {
				class = "value",
				text = function(): string
					local n = #lines.value
					return if n == 0 then "" else tostring(n)
				end,
			},
			on_click = function()
				shown.value = not shown.value
			end,
			rsb.popup {
				open = shown,
				name = "todo",
				class = "panel todo",
				placement = "below",
				align = "center",
				style = { width = 300 },
				function(): { any }
					return {
						rsb.column {
							class = "todo-body",
							rsb.input {
								class = "search",
								value = draft,
								placeholder = "Add a line",
								autofocus = true,
								on_submit = add,
							},
							rsb.scroll {
								class = "card list",
								style = { max_height = 240 },
								function(): { any }
									if #lines.value == 0 then
										return { rsb.text { class = "note", text = "Nothing to do" } }
									end
									return rows()
								end,
							},
						},
					}
				end,
			},
		}
	end,
}
