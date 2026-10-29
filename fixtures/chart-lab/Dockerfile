# syntax=docker/dockerfile:1.7
FROM kindest/node:v1.37.0@sha256:a1ed56cfb0e7b93589bdf97c8cd566405a265939e3620fc4f5de89adff580ae5
RUN printf '%s\n' \
      'Types: deb' \
      'URIs: http://snapshot.debian.org/archive/debian/20260803T000000Z' \
      'Suites: trixie' \
      'Components: main' \
      'Signed-By: /usr/share/keyrings/debian-archive-keyring.pgp' \
      > /etc/apt/sources.list.d/debian.sources && \
    apt-get -o Acquire::Check-Valid-Until=false update && \
    apt-get install -y --no-install-recommends apparmor=4.1.0-1 && \
    rm -rf /var/lib/apt/lists/* && \
    systemctl mask apparmor.service && \
    test "$(readlink /etc/systemd/system/apparmor.service)" = /dev/null && \
    test -x /sbin/apparmor_parser && \
    test "$(dpkg-query -W -f '${Version}' libapparmor1)" = 4.1.0-1
