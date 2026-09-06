//! 为大文件下载补单次网络读取的空闲上限；保留 ureq 的代理、TLS 与整体预算。

use ureq::unversioned::transport::{
    Buffers, ConnectionDetails, Connector, DefaultConnector, NextTimeout, Transport,
};

#[derive(Debug)]
pub(super) struct IdleConnector;

impl Connector for IdleConnector {
    type Out = IdleTransport;

    fn connect(
        &self,
        details: &ConnectionDetails,
        chained: Option<()>,
    ) -> Result<Option<Self::Out>, ureq::Error> {
        DefaultConnector::default()
            .connect(details, chained)
            .map(|transport| transport.map(|transport| IdleTransport(transport.boxed())))
    }
}

#[derive(Debug)]
pub(super) struct IdleTransport(Box<dyn Transport>);

impl Transport for IdleTransport {
    fn buffers(&mut self) -> &mut dyn Buffers {
        self.0.buffers()
    }
    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
        self.0.transmit_output(amount, timeout)
    }
    fn await_input(&mut self, mut timeout: NextTimeout) -> Result<bool, ureq::Error> {
        // 响应体整体预算可能长达一小时；每次等待上限 15s，使停滞与取消有界。
        // 持续有字节到达的慢下载不受此空闲限制。
        timeout.after = timeout
            .after
            .min(ureq::unversioned::transport::time::Duration::from_secs(15));
        self.0.await_input(timeout)
    }
    fn is_open(&mut self) -> bool {
        self.0.is_open()
    }
    fn is_tls(&self) -> bool {
        self.0.is_tls()
    }
}
