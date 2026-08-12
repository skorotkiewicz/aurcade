#!/usr/bin/env python3
import argparse
import base64
import imaplib
import os
import smtplib
import socket
import ssl
import time
import uuid
from email.message import EmailMessage

HOST = os.getenv("AURCADE_HOST", "127.0.0.1")
DOMAIN = os.environ["AURCADE_DOMAIN"]
TLS = os.getenv("AURCADE_TLS", "true") == "true"
USER_A = os.getenv("AURCADE_USER_A", "alice")
USER_B = os.getenv("AURCADE_USER_B", "bob")
PASSWORD_A = os.environ.get("AURCADE_PASSWORD_A", "")
PASSWORD_B = os.environ.get("AURCADE_PASSWORD_B", "")
CTX = ssl.create_default_context()
CTX.check_hostname = False
CTX.verify_mode = ssl.CERT_NONE


def connect(port, *, starttls=None):
    raw = socket.create_connection((HOST, port), 10)
    raw.settimeout(10)
    if TLS and starttls is None:
        return CTX.wrap_socket(raw, server_hostname=DOMAIN)
    return raw


def recv_until(sock, needles, timeout=15):
    if isinstance(needles, bytes):
        needles = [needles]
    data = b""
    end = time.time() + timeout
    while time.time() < end:
        try:
            chunk = sock.recv(65536)
        except socket.timeout:
            continue
        if not chunk:
            break
        data += chunk
        if any(needle in data for needle in needles):
            return data
    raise AssertionError(f"expected {needles!r}, received {data[-2000:]!r}")


def irc_login(port, user, password, nick, *, network=None):
    sock = connect(port)
    username = f"{user}/{network}" if network else user
    if network:
        sock.sendall(
            f"PASS {password}\r\nNICK {nick}\r\nUSER {username} 0 * :{user}\r\n".encode()
        )
    else:
        auth = base64.b64encode(f"\0{user}\0{password}".encode()).decode()
        sock.sendall(
            f"CAP LS 302\r\nNICK {nick}\r\nUSER {username} 0 * :{user}\r\n".encode()
        )
        recv_until(sock, b" LS :")
        sock.sendall(b"CAP REQ :sasl\r\n")
        recv_until(sock, [b" ACK sasl", b" ACK :sasl"])
        sock.sendall(b"AUTHENTICATE PLAIN\r\n")
        recv_until(sock, b"AUTHENTICATE +")
        sock.sendall(f"AUTHENTICATE {auth}\r\n".encode())
        recv_until(sock, [b" 903 ", b" 900 "])
        sock.sendall(b"CAP END\r\n")
    recv_until(sock, b" 001 ")
    return sock


def irc_rejects_bad_password(port, network=None):
    sock = connect(port)
    username = f"{USER_A}/{network}" if network else USER_A
    try:
        if network:
            sock.sendall(
                f"PASS definitely-wrong\r\nNICK badtest\r\nUSER {username} 0 * :badtest\r\n".encode()
            )
            recv_until(sock, [b" 464 ", b"FAIL", b"authentication failed"])
        else:
            auth = base64.b64encode(f"\0{USER_A}\0definitely-wrong".encode()).decode()
            sock.sendall(f"CAP LS 302\r\nNICK badtest\r\nUSER {username} 0 * :badtest\r\n".encode())
            recv_until(sock, b" LS :")
            sock.sendall(b"CAP REQ :sasl\r\n")
            recv_until(sock, [b" ACK sasl", b" ACK :sasl"])
            sock.sendall(b"AUTHENTICATE PLAIN\r\n")
            recv_until(sock, b"AUTHENTICATE +")
            sock.sendall(f"AUTHENTICATE {auth}\r\n".encode())
            recv_until(sock, [b" 904 ", b"FAIL"])
    finally:
        sock.close()


def irc(port, network=None):
    irc_rejects_bad_password(port, network)
    token = uuid.uuid4().hex[:10]
    channel = f"#test-{token}"
    a = irc_login(port, USER_A, PASSWORD_A, f"ta{token}", network=network)
    b = irc_login(port, USER_B, PASSWORD_B, f"tb{token}", network=network)
    try:
        a.sendall(f"JOIN {channel}\r\n".encode())
        b.sendall(f"JOIN {channel}\r\n".encode())
        recv_until(a, f"JOIN {channel}".encode())
        recv_until(b, f"JOIN {channel}".encode())
        a.sendall(f"PRIVMSG {channel} :{token}-a\r\n".encode())
        recv_until(b, [
            f"PRIVMSG {channel} :{token}-a".encode(),
            f"PRIVMSG {channel} {token}-a".encode(),
        ])
        b.sendall(f"PRIVMSG {channel} :{token}-b\r\n".encode())
        recv_until(a, [
            f"PRIVMSG {channel} :{token}-b".encode(),
            f"PRIVMSG {channel} {token}-b".encode(),
        ])
        a.sendall(f"PART {channel} :test complete\r\n".encode())
        b.sendall(f"PART {channel} :test complete\r\n".encode())
    finally:
        a.close()
        b.close()
    print(f"IRC message exchange passed on {port}")


def xml_open(sock, jid):
    sock.sendall(
        (f"<open xmlns='urn:ietf:params:xml:ns:xmpp-framing' to='{DOMAIN}' "
         "version='1.0'/>").encode()
    )
    return recv_until(sock, b"</stream:features>")


def xmpp_login(user, password):
    raw = socket.create_connection((HOST, 5222), 10)
    raw.settimeout(10)
    raw.sendall(
        (f"<stream:stream to='{DOMAIN}' version='1.0' "
         "xmlns='jabber:client' xmlns:stream='http://etherx.jabber.org/streams'>").encode()
    )
    features = recv_until(raw, b"</stream:features>")
    if TLS:
        assert b"starttls" in features
        raw.sendall(b"<starttls xmlns='urn:ietf:params:xml:ns:xmpp-tls'/>")
        recv_until(raw, b"<proceed")
        sock = CTX.wrap_socket(raw, server_hostname=DOMAIN)
        sock.settimeout(10)
        sock.sendall(
            (f"<stream:stream to='{DOMAIN}' version='1.0' "
             "xmlns='jabber:client' xmlns:stream='http://etherx.jabber.org/streams'>").encode()
        )
        recv_until(sock, b"</stream:features>")
    else:
        sock = raw
    auth = base64.b64encode(f"\0{user}\0{password}".encode()).decode()
    sock.sendall(
        f"<auth xmlns='urn:ietf:params:xml:ns:xmpp-sasl' mechanism='PLAIN'>{auth}</auth>".encode()
    )
    recv_until(sock, b"<success")
    sock.sendall(
        (f"<stream:stream to='{DOMAIN}' version='1.0' "
         "xmlns='jabber:client' xmlns:stream='http://etherx.jabber.org/streams'>").encode()
    )
    recv_until(sock, b"</stream:features>")
    sock.sendall(
        (f"<iq type='set' id='bind-{user}'><bind xmlns='urn:ietf:params:xml:ns:xmpp-bind'>"
         f"<resource>tests</resource></bind></iq>").encode()
    )
    bound = recv_until(sock, f"bind-{user}".encode())
    if b"type='result'" not in bound and b'type="result"' not in bound:
        bound += recv_until(sock, [b"type='result'", b'type="result"'])
    sock.sendall(b"<presence/>")
    return sock


def xmpp_rejects_bad_password():
    raw = socket.create_connection((HOST, 5222), 10)
    raw.settimeout(10)
    raw.sendall((f"<stream:stream to='{DOMAIN}' version='1.0' xmlns='jabber:client' xmlns:stream='http://etherx.jabber.org/streams'>").encode())
    features = recv_until(raw, b"</stream:features>")
    if TLS:
        assert b"starttls" in features
        raw.sendall(b"<starttls xmlns='urn:ietf:params:xml:ns:xmpp-tls'/>")
        recv_until(raw, b"<proceed")
        sock = CTX.wrap_socket(raw, server_hostname=DOMAIN)
        sock.settimeout(10)
        sock.sendall((f"<stream:stream to='{DOMAIN}' version='1.0' xmlns='jabber:client' xmlns:stream='http://etherx.jabber.org/streams'>").encode())
        recv_until(sock, b"</stream:features>")
    else:
        sock = raw
    auth = base64.b64encode(f"\0{USER_A}\0definitely-wrong".encode()).decode()
    sock.sendall(f"<auth xmlns='urn:ietf:params:xml:ns:xmpp-sasl' mechanism='PLAIN'>{auth}</auth>".encode())
    recv_until(sock, b"<failure")
    sock.close()


def xmpp():
    xmpp_rejects_bad_password()
    token = uuid.uuid4().hex
    a = xmpp_login(USER_A, PASSWORD_A)
    b = xmpp_login(USER_B, PASSWORD_B)
    try:
        a.sendall(
            f"<message to='{USER_B}@{DOMAIN}/tests' type='chat'><body>{token}</body></message>".encode()
        )
        recv_until(b, token.encode())
    finally:
        a.close()
        b.close()
    print("XMPP SASL login and message exchange passed")


def sieve_command(sock, command, expect=b"OK"):
    sock.sendall(command)
    reply = recv_until(sock, [b"OK ", b"OK\r\n", b"NO ", b"BYE "])
    assert expect in reply, reply
    return reply


def sieve_install(user, password, name, script):
    sock = socket.create_connection((HOST, 4190), 10)
    sock.settimeout(10)
    recv_until(sock, b"OK \"Dovecot ready.\"")
    if TLS:
        sock.sendall(b"STARTTLS\r\n")
        recv_until(sock, b"OK")
        sock = CTX.wrap_socket(sock, server_hostname=DOMAIN)
        sock.settimeout(10)
        recv_until(sock, b"OK \"Dovecot ready.\"")
    auth = base64.b64encode(f"\0{user}@{DOMAIN}\0{password}".encode()).decode()
    sieve_command(sock, f'AUTHENTICATE "PLAIN" "{auth}"\r\n'.encode())
    payload = script.encode()
    sieve_command(sock, f'PUTSCRIPT "{name}" {{{len(payload)}+}}\r\n'.encode() + payload + b"\r\n")
    sieve_command(sock, f'SETACTIVE "{name}"\r\n'.encode())
    sock.sendall(b"LOGOUT\r\n")
    sock.close()


def sieve_delete(user, password, name):
    sock = socket.create_connection((HOST, 4190), 10)
    sock.settimeout(10)
    recv_until(sock, b"OK \"Dovecot ready.\"")
    if TLS:
        sock.sendall(b"STARTTLS\r\n")
        recv_until(sock, b"OK")
        sock = CTX.wrap_socket(sock, server_hostname=DOMAIN)
        sock.settimeout(10)
        recv_until(sock, b"OK \"Dovecot ready.\"")
    auth = base64.b64encode(f"\0{user}@{DOMAIN}\0{password}".encode()).decode()
    sieve_command(sock, f'AUTHENTICATE "PLAIN" "{auth}"\r\n'.encode())
    sieve_command(sock, b'SETACTIVE ""\r\n')
    sieve_command(sock, f'DELETESCRIPT "{name}"\r\n'.encode())
    sock.sendall(b"LOGOUT\r\n")
    sock.close()


def smtp_client(user, password):
    if TLS:
        smtp = smtplib.SMTP(HOST, 587, timeout=15)
        smtp.ehlo()
        smtp.starttls(context=CTX)
        smtp.ehlo()
    else:
        smtp = smtplib.SMTP(HOST, 587, timeout=15)
    smtp.login(f"{user}@{DOMAIN}", password)
    return smtp


def smtp_send(user, password, recipient, subject, token):
    with smtp_client(user, password) as smtp:
        message = EmailMessage()
        message["From"] = f"{user}@{DOMAIN}"
        message["To"] = recipient
        message["Subject"] = subject
        message["Message-ID"] = f"<{token}@tests>"
        message.set_content(token)
        smtp.send_message(message)


def imap_find(user, password, mailbox, token):
    if TLS:
        imap = imaplib.IMAP4_SSL(HOST, 993, ssl_context=CTX)
    else:
        imap = imaplib.IMAP4(HOST, 993)
    try:
        imap.login(f"{user}@{DOMAIN}", password)
        status, _ = imap.select(mailbox)
        assert status == "OK"
        for _ in range(20):
            status, data = imap.search(None, "TEXT", f'"{token}"')
            if status == "OK" and data[0]:
                ids = data[0].split()
                status, body = imap.fetch(ids[-1], "(RFC822)")
                assert status == "OK" and token.encode() in body[0][1]
                imap.store(ids[-1], "+FLAGS", "\\Deleted")
                imap.expunge()
                return
            time.sleep(0.5)
        raise AssertionError(f"{token} not found in {mailbox}")
    finally:
        try:
            imap.logout()
        except Exception:
            pass


def mail():
    token = uuid.uuid4().hex
    script_name = f"test-{token}"
    subject = f"SIEVE-{token}"
    script = f'require ["fileinto", "mailbox"];\nif header :is "Subject" "{subject}" {{ fileinto :create "Archive"; stop; }}\n'
    try:
        if TLS:
            bad = smtplib.SMTP(HOST, 587, timeout=15)
            bad.ehlo()
            bad.starttls(context=CTX)
            bad.ehlo()
        else:
            bad = smtplib.SMTP(HOST, 587, timeout=15)
        try:
            bad.login(f"{USER_A}@{DOMAIN}", "definitely-wrong")
            raise AssertionError("SMTP accepted bad credentials")
        except smtplib.SMTPAuthenticationError:
            pass
        finally:
            try:
                bad.quit()
            except smtplib.SMTPServerDisconnected:
                pass
        with smtp_client(USER_A, PASSWORD_A) as smtp:
            code, _ = smtp.mail(f"{USER_A}@{DOMAIN}")
            assert code == 250
            code, _ = smtp.rcpt(f"missing-{token}@{DOMAIN}")
            assert code == 550
            smtp.rset()
        sieve_install(USER_B, PASSWORD_B, script_name, script)
        smtp_send(USER_A, PASSWORD_A, f"{USER_B}@{DOMAIN}", subject, token)
        imap_find(USER_B, PASSWORD_B, "Archive", token)
        reverse = f"reply-{token}"
        smtp_send(USER_B, PASSWORD_B, f"{USER_A}@{DOMAIN}", "normal delivery", reverse)
        imap_find(USER_A, PASSWORD_A, "INBOX", reverse)
        alias = os.getenv("AURCADE_MAIL_ALIAS")
        if alias:
            alias_token = f"alias-{token}"
            smtp_send(USER_B, PASSWORD_B, f"{alias}@{DOMAIN}", "alias delivery", alias_token)
            imap_find(USER_A, PASSWORD_A, "INBOX", alias_token)
    finally:
        try:
            sieve_delete(USER_B, PASSWORD_B, script_name)
        except Exception:
            pass
    print("SMTP submission, LMTP, Sieve Archive, IMAP receive, and reverse delivery passed")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=["irc", "soju", "xmpp", "mail"])
    args = parser.parse_args()
    if args.command == "irc":
        irc(6697)
    elif args.command == "soju":
        irc(6698, os.getenv("AURCADE_IRC_NETWORK", "AURcade"))
    elif args.command == "xmpp":
        xmpp()
    else:
        mail()


if __name__ == "__main__":
    main()
