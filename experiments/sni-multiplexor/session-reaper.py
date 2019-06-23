#!/usr/bin/env python
import sys
from datetime import timedelta
from operator import attrgetter
import subprocess

extra_types = dict()
extra_types['session_age_ms'] = int
extra_types['last_xmit_ago_ms'] = int

def load_sessions(fh):
    sessions = []
    for line in fh:
        line = line.strip()
        if line == "END":
            break
        parts = line.split(' ', 3)
        if len(parts) != 4:
            continue
        sess = Session()
        (session_id, state, local_sock_info, rest) = parts
        sess.session_id = session_id
        sess.state = state
        sess.local_sock_info = local_sock_info
        for kvpair in rest.split(' '):
            (key, value) = kvpair.split('=', 1)
            if key == 'session_age_ms':
                sess.session_age = timedelta_from_ms(int(value))
            elif key == 'last_xmit_ago_ms':
                sess.last_xmit_ago = timedelta_from_ms(int(value))
            elif key == 'backend_name':
                sess.backend = value
            else:
                exp_type = extra_types.get(key, None)
                if exp_type is not None:
                    sess.extras[key] = exp_type(value)
                else:
                    sess.extras[key] = value
        sessions.append(sess)
    return sessions

def get_sessions(path):
    subproc = subprocess.Popen(
        ['/bin/nc', '-U', path],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE)
    try:
        subproc.stdin.write("print-active-sessions\n")
        subproc.stdin.close()
        sessions = load_sessions(subproc.stdout)
    finally:
        subproc.wait()
    return sessions


def destroy_session(path, session_id):
    subproc = subprocess.Popen(
        ['/bin/nc', '-U', path],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE)
    try:
        subproc.stdin.write("destroy-session {}\n".format(session_id))
        subproc.stdin.close()
        response = subproc.stdout.read()
    finally:
        subproc.wait()


class Session(object):
    def __init__(self):
        self.session_id = None
        self.state = None
        self.local_sock_info = None
        self.extras = dict()
        self.session_age = None
        self.last_xmit_ago = None
        self.backend = None


def timedelta_from_ms(ms):
    return timedelta(ms / 86400000, (ms % 86400000) / 1000.0)


def print_session(sess):
    (local_bind, client_ip) = sess.local_sock_info.split(',', 1)
    print("{:28} {:16} {:15} {:15} {:20} {}".format(sess.session_id, sess.state, sess.session_age, sess.last_xmit_ago, sess.backend, client_ip))


def main():
    sessions = get_sessions('/var/run/sni-multiplexor-mgmt')
    for sess in sessions:
        if sess.state == 'shutdown-write' and timedelta(minutes=10) < sess.last_xmit_ago:
            print_session(sess)
            destroy_session('/var/run/sni-multiplexor-mgmt', sess.session_id)
    for sess in sessions:
        if sess.state == 'connected' and timedelta(hours=1) < sess.last_xmit_ago:
            print_session(sess)
            destroy_session('/var/run/sni-multiplexor-mgmt', sess.session_id)


if __name__ == '__main__':
    main()
