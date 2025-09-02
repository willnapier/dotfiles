-- Zotero Import Plugin for Yazi
-- Allows importing selected PDFs directly into Zotero

local function import_to_zotero(urls)
    local selected = {}
    for _, url in ipairs(urls) do
        table.insert(selected, tostring(url))
    end
    
    if #selected == 0 then
        ya.notify({
            title = "Zotero Import",
            content = "No files selected",
            timeout = 2,
        })
        return
    end
    
    -- Move files to Zotero auto-import directory
    local import_dir = os.getenv("HOME") .. "/Zotero/storage/import"
    os.execute("mkdir -p " .. import_dir)
    
    local imported_count = 0
    for _, file_path in ipairs(selected) do
        -- Only process PDF files
        if file_path:match("%.pdf$") or file_path:match("%.PDF$") then
            local filename = file_path:match("([^/]+)$")
            local cmd = string.format("cp '%s' '%s/%s'", file_path, import_dir, filename)
            
            if os.execute(cmd) == 0 then
                imported_count = imported_count + 1
                -- Mark original as processed by moving to processed folder
                os.execute(string.format("mv '%s' ~/Documents/ProcessedPDFs/", file_path))
            end
        end
    end
    
    if imported_count > 0 then
        ya.notify({
            title = "Zotero Import",
            content = string.format("Imported %d PDF(s) to Zotero", imported_count),
            timeout = 3,
        })
        
        -- Open Zotero if not running
        os.execute("pgrep -x zotero || open -a Zotero")
    else
        ya.notify({
            title = "Zotero Import",
            content = "No PDFs found in selection",
            timeout = 2,
        })
    end
end

return {
    entry = function(self, args)
        local urls = ya.selection()
        if #urls == 0 then
            urls = { ya.cursor() }
        end
        import_to_zotero(urls)
    end,
}