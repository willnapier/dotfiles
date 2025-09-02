local M = {}

local state = ya.sync(function()
	local selected = {}
	for _, url in pairs(cx.active.selected) do
		selected[#selected + 1] = url
	end
	return cx.active.current.cwd, selected
end)

function M:entry()
	ya.emit("escape", { visual = true })

	local _permit = ui.hide()
	local cwd, selected = state()

	local output, err = M.run_with(cwd, selected)
	if not output then
		return ya.notify { title = "Skim", content = tostring(err), timeout = 5, level = "error" }
	end

	local urls = M.split_urls(cwd, output)
	if #urls == 1 then
		local cha = #selected == 0 and fs.cha(urls[1])
		ya.emit(cha and cha.is_dir and "cd" or "reveal", { urls[1], raw = true })
	elseif #urls > 1 then
		urls.state = #selected > 0 and "off" or "on"
		ya.emit("toggle_all", urls)
	end
end

function M.run_with(cwd, selected)
	-- Prefer sk (skim) over fzf
	local cmd = "sk"
	local which = Command("which"):arg(cmd):output()
	if not which or which.status.code ~= 0 then
		cmd = "fzf"  -- Fallback to fzf if sk not found
	end
	
	local child, err = Command(cmd)
		:cwd(cwd)
		:env("FZF_DEFAULT_COMMAND", [[fd --hidden --type f --type d --exclude .git --exclude .Trash]])
		:stdin(selected and Command.INHERIT or Command.PIPED)
		:stdout(Command.PIPED)
		:stderr(Command.INHERIT)
		:spawn()

	if not child then
		return nil, Err("Failed to start `%s`, error: %s", cmd, err)
	end

	if not selected[1] then
		local output, err = child:wait()
		if not output then
			return nil, Err("Failed to wait `%s`, error: %s", cmd, err)
		elseif not output.status.success and output.status.code ~= 130 then
			return nil, Err("`%s` exited with error code %s", cmd, output.status.code)
		end
		return output.stdout
	end

	local items = {}
	for _, url in ipairs(selected) do
		if url:is_dir() then
			table.insert(items, " " .. url.name)
		else
			table.insert(items, string.format("[%s] %s", fs.size(url), url.name))
		end
	end

	local ok, err = child:write_all(table.concat(items, "\n") .. "\n")
	if not ok then
		return nil, Err("Failed to write to `%s`, error: %s", cmd, err)
	end

	local output, err = child:wait()
	if not output then
		return nil, Err("Cannot read `%s` output, error: %s", cmd, err)
	elseif not output.status.success and output.status.code ~= 130 then
		return nil, Err("`%s` exited with error code %s", cmd, output.status.code)
	end
	return output.stdout
end

function M.split_urls(cwd, output)
	local urls = {}
	for line in output:gmatch("[^\r\n]+") do
		if line:sub(1, 1) == " " then
			urls[#urls + 1] = Url(cwd .. "/" .. line:sub(2))
		else
			urls[#urls + 1] = Url(cwd .. "/" .. line:match("%] (.+)"))
		end
	end
	return urls
end

return M