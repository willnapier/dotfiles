-- Zotero Import Plugin for Yazi
-- Quickly send PDFs and research files to Zotero's Import folder

local function notify(msg, level)
	level = level or "info"
	ya.notify({
		title = "Zotero",
		content = msg,
		level = level,
		timeout = 3,
	})
end

local function import_to_zotero(state, args)
	local zotero_import = os.getenv("HOME") .. "/Zotero/Import"
	
	-- Check if Import folder exists
	local stat = ya.fs_stat(zotero_import)
	if not stat or not stat.is_dir then
		notify("Zotero Import folder not found at " .. zotero_import, "error")
		return
	end
	
	-- Get selected files or current file
	local selected = state.files
	if #selected == 0 then
		local hovered = cx.active.current.hovered
		if hovered then
			selected = { hovered }
		else
			notify("No files selected", "warn")
			return
		end
	end
	
	-- Process each selected file
	local imported = {}
	local failed = {}
	
	for _, file in ipairs(selected) do
		if file.cha.is_file then
			local filename = file.name
			local source_path = tostring(file.url)
			local dest_path = zotero_import .. "/" .. filename
			
			-- Check if file already exists in Import folder
			local dest_stat = ya.fs_stat(dest_path)
			if dest_stat then
				-- Add timestamp to filename to avoid conflicts
				local name, ext = filename:match("^(.+)%.([^%.]+)$")
				if name and ext then
					filename = name .. "_" .. os.time() .. "." .. ext
				else
					filename = filename .. "_" .. os.time()
				end
				dest_path = zotero_import .. "/" .. filename
			end
			
			-- Copy file to Import folder
			local success = os.execute(string.format('cp "%s" "%s"', source_path, dest_path))
			if success == 0 then
				table.insert(imported, filename)
			else
				table.insert(failed, filename)
			end
		end
	end
	
	-- Show results
	if #imported > 0 then
		if #imported == 1 then
			notify("Imported: " .. imported[1], "info")
		else
			notify(string.format("Imported %d files to Zotero", #imported), "info")
		end
	end
	
	if #failed > 0 then
		notify(string.format("Failed to import %d files", #failed), "error")
	end
end

local function move_to_zotero(state, args)
	local zotero_import = os.getenv("HOME") .. "/Zotero/Import"
	
	-- Check if Import folder exists
	local stat = ya.fs_stat(zotero_import)
	if not stat or not stat.is_dir then
		notify("Zotero Import folder not found at " .. zotero_import, "error")
		return
	end
	
	-- Get selected files or current file
	local selected = state.files
	if #selected == 0 then
		local hovered = cx.active.current.hovered
		if hovered then
			selected = { hovered }
		else
			notify("No files selected", "warn")
			return
		end
	end
	
	-- Process each selected file
	local moved = {}
	local failed = {}
	
	for _, file in ipairs(selected) do
		if file.cha.is_file then
			local filename = file.name
			local source_path = tostring(file.url)
			local dest_path = zotero_import .. "/" .. filename
			
			-- Check if file already exists in Import folder
			local dest_stat = ya.fs_stat(dest_path)
			if dest_stat then
				-- Add timestamp to filename to avoid conflicts
				local name, ext = filename:match("^(.+)%.([^%.]+)$")
				if name and ext then
					filename = name .. "_" .. os.time() .. "." .. ext
				else
					filename = filename .. "_" .. os.time()
				end
				dest_path = zotero_import .. "/" .. filename
			end
			
			-- Move file to Import folder
			local success = os.execute(string.format('mv "%s" "%s"', source_path, dest_path))
			if success == 0 then
				table.insert(moved, filename)
			else
				table.insert(failed, filename)
			end
		end
	end
	
	-- Show results
	if #moved > 0 then
		if #moved == 1 then
			notify("Moved: " .. moved[1], "info")
		else
			notify(string.format("Moved %d files to Zotero", #moved), "info")
		end
	end
	
	if #failed > 0 then
		notify(string.format("Failed to move %d files", #failed), "error")
	end
	
	-- Refresh current directory
	ya.manager_emit("reload", { force = true })
end

local function open_import_folder(state, args)
	local zotero_import = os.getenv("HOME") .. "/Zotero/Import"
	ya.manager_emit("cd", { zotero_import })
end

return {
	entry = function(state, args)
		local action = args[1] or "import"
		
		if action == "import" then
			import_to_zotero(state, args)
		elseif action == "move" then
			move_to_zotero(state, args)
		elseif action == "open" then
			open_import_folder(state, args)
		else
			notify("Unknown action: " .. action, "error")
		end
	end,
}