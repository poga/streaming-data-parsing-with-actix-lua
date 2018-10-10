return function (msg)
    print("handle")
    local data = json.decode(msg)

    for i, v in pairs(data["items"]) do
        if string.find(v["name"], "Belly of the Beast") then
            print(data["accountName"])
        end
    end
end