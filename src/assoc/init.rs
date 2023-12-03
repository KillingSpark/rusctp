use std::time::Instant;

use bytes::{Buf, Bytes};
use rand::RngCore;

use crate::{
    packet::{
        cookie::{Cookie, StateCookie},
        init::{InitAck, InitChunk},
        Chunk, Packet, Tsn, UnrecognizedParam,
    },
    AssocAlias, AssocId, FakeAddr, PerAssocInfo, Sctp, TransportAddress, WaitInitAck,
};

use super::{AssocTxSettings, Association, TxNotification};

// Code for actively initializing an association

pub enum HandleSpecialResult<T> {
    NotRecognized,
    Handled(T),
    Error,
}

impl<T> HandleSpecialResult<T> {
    pub fn handled_or_error(&self) -> bool {
        matches!(
            self,
            HandleSpecialResult::Handled(_) | HandleSpecialResult::Error
        )
    }
}

impl<FakeContent: FakeAddr> Sctp<FakeContent> {
    pub fn init_association(
        &mut self,
        peer_addr: TransportAddress<FakeContent>,
        peer_port: u16,
        local_port: u16,
    ) {
        let alias = AssocAlias {
            peer_addr,
            peer_port,
            local_port,
        };
        let local_verification_tag = rand::thread_rng().next_u32();
        let initial_tsn = rand::thread_rng().next_u32();
        self.wait_init_ack.insert(
            alias,
            WaitInitAck {
                local_verification_tag,
                local_initial_tsn: initial_tsn,
            },
        );

        let init_chunk = Chunk::Init(self.create_init_chunk(local_verification_tag, initial_tsn));

        let packet = Packet::new(local_port, peer_port, 0);
        self.send_immediate
            .push_back((peer_addr, packet, init_chunk))
    }

    fn create_init_chunk(
        &mut self,
        local_verification_tag: u32,
        initial_tsn: u32,
    ) -> InitChunk<FakeContent> {
        InitChunk {
            initiate_tag: local_verification_tag,
            a_rwnd: self.settings.in_buffer_limit as u32,
            outbound_streams: self.settings.outgoing_streams,
            inbound_streams: self.settings.outgoing_streams,
            initial_tsn,
            unrecognized: vec![],
            aliases: vec![],
            cookie_preservative: None,
            supported_addr_types: None,
        }
    }

    /// Returns true if the packet should not be processed further
    pub(crate) fn handle_init_ack(
        &mut self,
        packet: &Packet,
        data: &mut Bytes,
        from: TransportAddress<FakeContent>,
    ) -> HandleSpecialResult<()> {
        if !Chunk::<FakeContent>::is_init_ack(data) {
            return HandleSpecialResult::NotRecognized;
        }

        let (size, Ok(Chunk::InitAck(init_ack))) = Chunk::parse(data) else {
            return HandleSpecialResult::Error;
        };

        if size != data.len() {
            // This is illegal, the init_ack needs to be the only chunk in the packet
            // -> stop processing this
            return HandleSpecialResult::Error;
        }
        let alias = AssocAlias {
            peer_addr: from,
            peer_port: packet.from(),
            local_port: packet.to(),
        };
        let Some(half_open) = self.wait_init_ack.remove(&alias) else {
            return HandleSpecialResult::Error;
        };
        self.wait_cookie_ack.insert(
            alias,
            crate::WaitCookieAck {
                peer_verification_tag: init_ack.initiate_tag,
                local_verification_tag: half_open.local_verification_tag,
                aliases: init_ack.aliases,
                original_address: from,
                local_initial_tsn: half_open.local_initial_tsn,
                peer_initial_tsn: init_ack.initial_tsn,
                local_in_streams: self.settings.incoming_streams,
                peer_in_streams: init_ack.inbound_streams,
                local_out_streams: self.settings.outgoing_streams,
                peer_out_streams: init_ack.outbound_streams,
                peer_arwnd: init_ack.a_rwnd,
            },
        );
        self.send_immediate.push_back((
            from,
            Packet::new(packet.to(), packet.from(), init_ack.initiate_tag),
            Chunk::StateCookie(init_ack.cookie),
        ));
        HandleSpecialResult::Handled(())
    }

    pub(crate) fn handle_cookie_ack(
        &mut self,
        packet: &Packet,
        data: &mut Bytes,
        from: TransportAddress<FakeContent>,
        now: Instant,
    ) -> HandleSpecialResult<AssocId> {
        if !Chunk::<FakeContent>::is_cookie_ack(data) {
            return HandleSpecialResult::NotRecognized;
        }
        let (size, Ok(Chunk::StateCookieAck)) = Chunk::<FakeContent>::parse(data) else {
            return HandleSpecialResult::Error;
        };
        data.advance(size);

        if let Some(existing) = self.aliases.get(&AssocAlias {
            peer_addr: from,
            peer_port: packet.from(),
            local_port: packet.to(),
        }) {
            // TODO handle this better. Technically we have a new association here
            // We might get away with just updating the verification tag and expected tsns in the RX/TX?
            return HandleSpecialResult::Handled(*existing);
        }

        let Some(half_open) = self.wait_cookie_ack.remove(&AssocAlias {
            peer_addr: from,
            peer_port: packet.from(),
            local_port: packet.to(),
        }) else {
            return HandleSpecialResult::Error;
        };

        let assoc_id = self.make_new_assoc(
            packet,
            &half_open.aliases,
            half_open.local_verification_tag,
            half_open.peer_initial_tsn,
            u16::min(half_open.local_in_streams, half_open.peer_in_streams),
            AssocTxSettings {
                primary_path: half_open.original_address,
                peer_verification_tag: half_open.peer_verification_tag,
                local_port: packet.to(),
                peer_port: packet.from(),
                init_tsn: Tsn(half_open.local_initial_tsn),
                out_streams: u16::min(half_open.local_out_streams, half_open.peer_out_streams),
                out_buffer_limit: self.settings.out_buffer_limit,
                peer_arwnd: half_open.peer_arwnd,
                pmtu: self.settings.pmtu,
            },
            now,
        );
        HandleSpecialResult::Handled(assoc_id)
    }

    /// Returns true if the packet should not be processed further
    pub(crate) fn handle_init(
        &mut self,
        packet: &Packet,
        data: &Bytes,
        from: TransportAddress<FakeContent>,
    ) -> HandleSpecialResult<()> {
        if !Chunk::<FakeContent>::is_init(data) {
            return HandleSpecialResult::NotRecognized;
        }
        let (size, Ok(Chunk::Init(init))) = Chunk::parse(data) else {
            return HandleSpecialResult::Error;
        };
        if size != data.len() {
            // This is illegal, the init needs to be the only chunk in the packet
            // -> stop processing this
            return HandleSpecialResult::Error;
        }

        let initial_tsn;
        let local_verification_tag;
        if let Some(half_open) = self.wait_init_ack.get(&AssocAlias {
            peer_addr: from,
            peer_port: packet.from(),
            local_port: packet.to(),
        }) {
            // Init overlap, send another init_ack with the same parameters as the init we already sent
            initial_tsn = half_open.local_initial_tsn;
            local_verification_tag = half_open.local_verification_tag;
        } else {
            // Answer with newly chosen random values
            initial_tsn = rand::thread_rng().next_u32();
            local_verification_tag = rand::thread_rng().next_u32();
        }

        let mut cookie = Cookie {
            init_address: from,
            aliases: init.aliases,
            peer_port: packet.from(),
            local_port: packet.to(),
            local_verification_tag,
            peer_verification_tag: init.initiate_tag,
            local_initial_tsn: initial_tsn,
            peer_initial_tsn: init.initial_tsn,
            incoming_streams: u16::min(self.settings.incoming_streams, init.outbound_streams),
            outgoing_streams: u16::min(self.settings.outgoing_streams, init.inbound_streams),
            peer_arwnd: init.a_rwnd,
            mac: 0,
        };
        cookie.mac = cookie.calc_mac(&self.settings.cookie_secret);
        let cookie = StateCookie::Ours(cookie);
        let init_ack = Chunk::InitAck(self.create_init_ack(
            init.unrecognized,
            cookie,
            initial_tsn,
            local_verification_tag,
        ));
        self.send_immediate.push_back((
            from,
            Packet::new(packet.to(), packet.from(), init.initiate_tag),
            init_ack,
        ));
        // handled the init correctly, no need to process the packet any further
        HandleSpecialResult::Handled(())
    }

    fn create_init_ack(
        &mut self,
        unrecognized: Vec<UnrecognizedParam>,
        cookie: StateCookie<FakeContent>,
        initial_tsn: u32,
        local_verification_tag: u32,
    ) -> InitAck<FakeContent> {
        InitAck {
            initiate_tag: local_verification_tag,
            a_rwnd: self.settings.in_buffer_limit as u32,
            outbound_streams: self.settings.outgoing_streams,
            inbound_streams: self.settings.incoming_streams,
            initial_tsn,
            cookie,
            unrecognized,
            aliases: vec![],
            cookie_preservative: None,
            supported_addr_types: None,
        }
    }

    pub(crate) fn handle_cookie_echo(
        &mut self,
        packet: &Packet,
        data: &mut Bytes,
        from: TransportAddress<FakeContent>,
        now: Instant,
    ) -> HandleSpecialResult<AssocId> {
        if !Chunk::<FakeContent>::is_cookie_echo(data) {
            return HandleSpecialResult::NotRecognized;
        }
        let (size, Ok(Chunk::StateCookie(mut cookie))) = Chunk::parse(data) else {
            return HandleSpecialResult::Error;
        };
        let Some(cookie) = cookie.make_ours() else {
            // TODO this is malicious, maybe do something more drastic?
            return HandleSpecialResult::Error;
        };

        let calced_mac = cookie.calc_mac(&self.settings.cookie_secret);

        if calced_mac != cookie.mac {
            // TODO this is malicious, maybe do something more drastic?
            return HandleSpecialResult::Error;
        }

        data.advance(size);

        if let Some(existing) = self.aliases.get(&AssocAlias {
            peer_addr: from,
            peer_port: packet.from(),
            local_port: packet.to(),
        }) {
            // TODO handle this better. Technically we have a new association here
            // We might get away with just updating the verification tag and expected tsns in the RX/TX?
            return HandleSpecialResult::Handled(*existing);
        }

        let assoc_id = self.make_new_assoc(
            packet,
            &cookie.aliases,
            cookie.local_verification_tag,
            cookie.peer_initial_tsn,
            cookie.incoming_streams,
            AssocTxSettings {
                primary_path: cookie.init_address,
                peer_verification_tag: cookie.peer_verification_tag,
                local_port: cookie.local_port,
                peer_port: cookie.peer_port,
                init_tsn: Tsn(cookie.local_initial_tsn),
                out_streams: cookie.outgoing_streams,
                out_buffer_limit: self.settings.out_buffer_limit,
                peer_arwnd: cookie.peer_arwnd,
                pmtu: self.settings.pmtu,
            },
            now,
        );
        self.tx_notifications
            .push_back((assoc_id, TxNotification::Send(Chunk::StateCookieAck)));
        HandleSpecialResult::Handled(assoc_id)
    }

    #[allow(clippy::too_many_arguments)]
    fn make_new_assoc(
        &mut self,
        packet: &Packet,
        alias_addresses: &[TransportAddress<FakeContent>],
        local_verification_tag: u32,
        peer_initial_tsn: u32,
        incoming_streams: u16,
        tx_settings: AssocTxSettings<FakeContent>,
        now: Instant,
    ) -> AssocId {
        let assoc_id = self.next_assoc_id();
        self.assoc_infos.insert(
            assoc_id,
            PerAssocInfo {
                local_verification_tag,
                peer_verification_tag: tx_settings.peer_verification_tag,
            },
        );
        let original_alias = AssocAlias {
            peer_addr: tx_settings.primary_path,
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
            Tsn(peer_initial_tsn),
            incoming_streams,
            self.settings.in_buffer_limit,
            tx_settings,
            now,
        ));
        assoc_id
    }

    fn next_assoc_id(&mut self) -> AssocId {
        self.assoc_id_gen += 1;
        AssocId(self.assoc_id_gen)
    }
}
