use bytes::{Buf, Bytes};

use crate::{
    packet::{
        cookie::{Cookie, StateCookie},
        init::{InitAck, InitChunk},
        Chunk, Packet, UnrecognizedParam,
    },
    AssocAlias, AssocId, PerAssocInfo, Sctp, TransportAddress, WaitInitAck,
};

use super::{Association, TxNotification};

// Code for actively initializing an association

impl Sctp {
    pub fn init_association(
        &mut self,
        peer_addr: TransportAddress,
        peer_port: u16,
        local_port: u16,
    ) {
        let alias = AssocAlias {
            peer_addr,
            peer_port,
            local_port,
        };
        self.wait_init_ack.insert(
            alias,
            WaitInitAck {
                local_verification_tag: 1337, // TODO
            },
        );

        let init_chunk = Chunk::Init(self.create_init_chunk());

        let packet = Packet::new(peer_port, local_port, 1337 /* TODO */);
        self.send_immediate
            .push_back((peer_addr, packet, init_chunk))
    }

    fn create_init_chunk(&self) -> InitChunk {
        todo!()
    }

    /// Returns true if the packet should not be processed further
    pub(crate) fn handle_init_ack(
        &mut self,
        packet: &Packet,
        data: &mut Bytes,
        from: TransportAddress,
    ) -> bool {
        if !Chunk::is_init_ack(data) {
            return true;
        }
        let (size, Ok(chunk)) = Chunk::parse(data) else {
            return true;
        };
        if let Chunk::InitAck(init_ack) = chunk {
            if size != data.len() {
                // This is illegal, the init_ack needs to be the only chunk in the packet
                // -> stop processing this
                return true;
            }
            let alias = AssocAlias {
                peer_addr: from,
                peer_port: packet.from(),
                local_port: packet.to(),
            };
            let Some(half_open) = self.wait_init_ack.remove(&alias) else {
                return true;
            };
            self.wait_cookie_ack.insert(
                alias,
                crate::WaitCookieAck {
                    peer_verification_tag: init_ack.initiate_tag,
                    local_verification_tag: half_open.local_verification_tag,
                    aliases: init_ack.aliases,
                    original_address: from,
                },
            );
            self.send_immediate.push_back((
                from,
                Packet::new(packet.to(), packet.from(), init_ack.initiate_tag),
                Chunk::StateCookie(init_ack.cookie),
            ));
            true
        } else {
            false
        }
    }

    pub(crate) fn handle_cookie_ack(
        &mut self,
        packet: &Packet,
        data: &mut Bytes,
        from: TransportAddress,
    ) -> Option<AssocId> {
        if !Chunk::is_init_ack(data) {
            return None;
        }
        let (size, Ok(chunk)) = Chunk::parse(data) else {
            return None;
        };
        if let Chunk::StateCookieAck = chunk {
            data.advance(size);
            let half_open = self.wait_cookie_ack.remove(&AssocAlias {
                peer_addr: from,
                peer_port: packet.from(),
                local_port: packet.to(),
            })?;
            let assoc_id = self.make_new_assoc(
                packet,
                half_open.original_address,
                &half_open.aliases,
                half_open.local_verification_tag,
                half_open.peer_verification_tag,
            );
            Some(assoc_id)
        } else {
            None
        }
    }

    /// Returns true if the packet should not be processed further
    pub(crate) fn handle_init(
        &mut self,
        packet: &Packet,
        data: &Bytes,
        from: TransportAddress,
    ) -> bool {
        if !Chunk::is_init(data) {
            return false;
        }
        let (size, Ok(chunk)) = Chunk::parse(data) else {
            // Does not parse correctly.
            // Handling this correctly is done in process_chunks.
            return false;
        };
        if let Chunk::Init(init) = chunk {
            if size != data.len() {
                // This is illegal, the init needs to be the only chunk in the packet
                // -> stop processing this
                return true;
            }
            let mac = Cookie::calc_mac(
                from,
                &init.aliases,
                packet.from(),
                packet.to(),
                &self.cookie_secret,
            );
            let cookie = StateCookie::Ours(Cookie {
                init_address: from,
                aliases: init.aliases,
                peer_port: packet.from(),
                local_port: packet.to(),
                local_verification_tag: 1337, // TODO
                peer_verification_tag: init.initiate_tag,
                mac,
            });
            let init_ack = Chunk::InitAck(self.create_init_ack(init.unrecognized, cookie));
            self.send_immediate.push_back((
                from,
                Packet::new(packet.to(), packet.from(), init.initiate_tag),
                init_ack,
            ));
            // handled the init correctly, no need to process the packet any further
            true
        } else {
            unreachable!("We checked above that this is an init chunk")
        }
    }

    fn create_init_ack(
        &self,
        unrecognized: Vec<UnrecognizedParam>,
        _cookie: StateCookie,
    ) -> InitAck {
        _ = unrecognized;
        unimplemented!()
    }

    pub(crate) fn handle_cookie_echo(
        &mut self,
        packet: &Packet,
        data: &mut Bytes,
    ) -> Option<AssocId> {
        if !Chunk::is_cookie_echo(data) {
            return None;
        }
        let (size, Ok(chunk)) = Chunk::parse(data) else {
            return None;
        };
        let Chunk::StateCookie(mut cookie) = chunk else {
            return None;
        };
        let Some(cookie) = cookie.make_ours() else {
            return None;
        };

        let calced_mac = Cookie::calc_mac(
            cookie.init_address,
            &cookie.aliases,
            cookie.peer_port,
            cookie.local_port,
            &self.cookie_secret,
        );

        if calced_mac != cookie.mac {
            // TODO maybe bail more drastically?
            return None;
        }

        data.advance(size);
        let assoc_id = self.make_new_assoc(
            packet,
            cookie.init_address,
            &cookie.aliases,
            cookie.local_verification_tag,
            cookie.peer_verification_tag,
        );
        self.tx_notifications
            .push_back((assoc_id, TxNotification::Send(Chunk::StateCookieAck)));
        Some(assoc_id)
    }

    fn make_new_assoc(
        &mut self,
        packet: &Packet,
        init_address: TransportAddress,
        alias_addresses: &[TransportAddress],
        local_verification_tag: u32,
        peer_verification_tag: u32,
    ) -> AssocId {
        let assoc_id = self.next_assoc_id();
        self.assoc_infos.insert(
            assoc_id,
            PerAssocInfo {
                local_verification_tag,
                _peer_verification_tag: peer_verification_tag,
            },
        );
        let original_alias = AssocAlias {
            peer_addr: init_address,
            peer_port: packet.from(),
            local_port: packet.to(),
        };
        self.aliases.insert(original_alias, assoc_id);
        for alias_addr in alias_addresses {
            let mut alias = original_alias;
            alias.peer_addr = *alias_addr;
            self.aliases.insert(alias, assoc_id);
        }
        self.new_assoc = Some(Association::new(
            assoc_id,
            init_address,
            packet.verification_tag(),
            packet.to(),
            packet.from(),
        ));
        assoc_id
    }

    fn next_assoc_id(&mut self) -> AssocId {
        self.assoc_id_gen += 1;
        AssocId(self.assoc_id_gen)
    }
}
