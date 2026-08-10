local http = require "socket.http";
local ltn12 = require "ltn12";
local new_sasl = require "prosody.util.sasl".new;

local auth_url = module:get_option_string("aurcade_auth_url", "http://aurcade:9000");

local function request(path, username, password)
	local _, code = http.request({
		url = auth_url .. path;
		method = "POST";
		headers = {
			["X-Aurcade-Account"] = username;
			["Content-Length"] = tostring(#password);
		};
		source = ltn12.source.string(password);
		sink = ltn12.sink.table({});
	});
	return code == 204;
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
