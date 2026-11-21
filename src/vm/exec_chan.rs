use std::sync::Arc;

use crate::native::chan::{
    CHAN_CAP, CHAN_CLOSE, CHAN_IS_CLOSED, CHAN_IS_EMPTY, CHAN_LEN, CHAN_OPEN, CHAN_RECV,
    CHAN_RECV_TIMEOUT, CHAN_SEND, CHAN_TRY_RECV, CHAN_TRY_SEND, ChannelState, get_channel,
    make_chan_handle, open_channel,
};
use crate::vm::exec_cold::err_msg;
use crate::vm::value::{Obj, SCHEMA_CHAN_HANDLE};
use crate::vm::{RuntimeError, Vm};

impl Vm {
    fn extract_channel(
        &self,
        o_base: usize,
        reg: u16,
        ctx: &str,
    ) -> Result<Arc<ChannelState>, RuntimeError> {
        let Obj::Record(rec) = &self.objs[o_base + reg as usize] else {
            return Err(err_msg(&format!("Expected Chan in {}", ctx)));
        };
        if rec.schema_id != SCHEMA_CHAN_HANDLE {
            return Err(err_msg(&format!("Expected Chan in {}", ctx)));
        }
        get_channel(rec.get_scalar(0))
            .ok_or_else(|| err_msg(&format!("Invalid Chan handle in {}", ctx)))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn exec_chan_op(
        &mut self,
        s_base: usize,
        o_base: usize,
        aux: u8,
        a: u16,
        b: u16,
        c: u16,
    ) -> Result<(), RuntimeError> {
        match aux {
            CHAN_OPEN => {
                let cap = self.scalars[s_base + b as usize] as i64;
                let id = open_channel(cap);
                self.objs[o_base + a as usize] = make_chan_handle(id);
            }
            CHAN_SEND => {
                let ch = self.extract_channel(o_base, b, "Chan.send()")?;
                let val = self.objs[o_base + c as usize].clone();
                let sent = ch.send(val);
                self.scalars[s_base + a as usize] = u64::from(sent);
            }
            CHAN_TRY_SEND => {
                let ch = self.extract_channel(o_base, b, "Chan.try_send()")?;
                let val = self.objs[o_base + c as usize].clone();
                let sent = ch.try_send(val);
                self.scalars[s_base + a as usize] = u64::from(sent);
            }
            CHAN_RECV => {
                let ch = self.extract_channel(o_base, b, "Chan.recv()")?;
                self.objs[o_base + a as usize] = ch.recv().unwrap_or(Obj::Empty);
            }
            CHAN_TRY_RECV => {
                let ch = self.extract_channel(o_base, b, "Chan.try_recv()")?;
                self.objs[o_base + a as usize] = ch.try_recv().unwrap_or(Obj::Empty);
            }
            CHAN_RECV_TIMEOUT => {
                let ch = self.extract_channel(o_base, b, "Chan.recv_timeout()")?;
                let timeout_ms = self.scalars[s_base + c as usize] as i64;
                self.objs[o_base + a as usize] =
                    ch.recv_timeout(timeout_ms).unwrap_or(Obj::Empty);
            }
            CHAN_CLOSE => {
                let ch = self.extract_channel(o_base, b, "Chan.close()")?;
                ch.close();
            }
            CHAN_LEN => {
                let ch = self.extract_channel(o_base, b, "Chan.len()")?;
                self.scalars[s_base + a as usize] = ch.len() as u64;
            }
            CHAN_CAP => {
                let ch = self.extract_channel(o_base, b, "Chan.cap()")?;
                self.scalars[s_base + a as usize] = ch.cap() as u64;
            }
            CHAN_IS_EMPTY => {
                let ch = self.extract_channel(o_base, b, "Chan.is_empty()")?;
                self.scalars[s_base + a as usize] = u64::from(ch.is_empty());
            }
            CHAN_IS_CLOSED => {
                let ch = self.extract_channel(o_base, b, "Chan.is_closed()")?;
                self.scalars[s_base + a as usize] = u64::from(ch.is_closed());
            }
            _ => return Err(err_msg(&format!("Unknown cdt.chan op {}", aux))),
        }
        Ok(())
    }
}
