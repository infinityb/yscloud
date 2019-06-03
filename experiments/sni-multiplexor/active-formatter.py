#!/usr/bin/env python
import sys
from datetime import timedelta
from operator import attrgetter

extra_types = dict()
extra_types['session_age_ms'] = int
extra_types['last_xmit_ago_ms'] = int

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
    print("{:28} {:16} {:15} {:15} {:20} {}".format("session_id", "state", "session_age", "last_xmit_ago", "backend", "client ip"))
    sessions = []
    for line in sys.stdin:
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
    for sess in sorted_from_expr(sessions, sys.argv[1]):
        print_session(sess)


def sorted_from_expr(v, expr):
    should_reverse=False
    if expr.startswith('-'):
        expr = expr[1:]
        should_reverse = True
    return sorted(v, key=attrgetter(expr), reverse=should_reverse)

if __name__ == '__main__':
    main()
