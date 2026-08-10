local socket = require "socket";
local new_sasl = require "prosody.util.sasl".new;

local auth_host = module:get_option_string("aurcade_auth_host", "aurcade");
local auth_port = module:get_option_integer("aurcade_auth_port", 9000, 1, 65535);

local function request(path, username, password)
	if not username:match("^[A-Za-z0-9_-]+$") then return false; end
	local client = socket.tcp();
	client:settimeout(10);
	if not client:connect(auth_host, auth_port) then return false; end
	local ok = client:send((
		"POST %s HTTP/1.1\r\nHost: %s\r\nX-Aurcade-Account: %s\r\nContent-Length: %d\r\nConnection: close\r\n\r\n%s"
	):format(path, auth_host, username, #password, password));
	if not ok then client:close(); return false; end
	local status = client:receive("*l");
	client:close();
	return status and status:match("^HTTP/1%.1 204 ") ~= nil;
end

local provider = {};

function provider.test_password(username, password)
	return request("/verify", username, password);
end

function provider.user_exists(username)
	return request("/exists", username, "");
end

function provider.create_user()
	return nil, "Account creation is managed by AURcade";
end

function provider.set_password()
	return nil, "Passwords are managed by AURcade";
end

function provider.get_sasl_handler()
	return new_sasl(module.host, {
		plain_test = function(_, username, password)
			return provider.test_password(username, password), true;
		end;
	});
end

module:provides("auth", provider);
