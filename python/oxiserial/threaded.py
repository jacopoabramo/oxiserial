# Derived from pyserial's serial/threaded (BSD-3-Clause, see LICENSES/pyserial.txt).
"""Threads that read a serial port and pass the data to a protocol object."""

import threading
from collections.abc import Callable
from types import TracebackType
from typing import TYPE_CHECKING, Any, ClassVar, Generic, Self, TypeVar

from oxiserial import Serial, SerialException

if TYPE_CHECKING:
    from typing_extensions import Buffer

__all__ = ["FramedPacket", "LineReader", "Packetizer", "Protocol", "ReaderThread"]


class Protocol:
    """Callbacks a [`ReaderThread`][oxiserial.threaded.ReaderThread] makes.

    Each does nothing here, except `connection_lost`, which raises the error
    that ended the loop.
    """

    def connection_made(self, transport: "ReaderThread[Any]") -> None:
        """Run in the reader thread once it starts."""

    def data_received(self, data: bytes) -> None:
        """Receive bytes as they arrive from the port."""

    def connection_lost(self, exc: Exception | None) -> None:
        """Run once the reader loop has ended.

        Raises
        ------
        Exception
            `exc`, when the loop ended because of an error.
        """
        if exc is not None:
            raise exc


class Packetizer(Protocol):
    """Split the received bytes into packets that end with `TERMINATOR`."""

    TERMINATOR: ClassVar[bytes] = b"\0"

    def __init__(self) -> None:
        self.buffer = bytearray()
        self.transport: ReaderThread[Any] | None = None

    def connection_made(self, transport: "ReaderThread[Any]") -> None:
        """Keep `transport`."""
        self.transport = transport

    def connection_lost(self, exc: Exception | None) -> None:
        """Forget the transport, then raise `exc` if it is set."""
        self.transport = None
        super().connection_lost(exc)

    def data_received(self, data: bytes) -> None:
        """Add `data` to `buffer` and pass each complete packet to `handle_packet`."""
        self.buffer.extend(data)
        while self.TERMINATOR in self.buffer:
            packet, self.buffer = self.buffer.split(self.TERMINATOR, 1)
            self.handle_packet(packet)

    def handle_packet(self, packet: bytearray) -> None:
        """Process one packet, without its terminator.

        Raises
        ------
        NotImplementedError
            Unless a subclass overrides it.
        """
        raise NotImplementedError("please implement functionality in handle_packet")


class FramedPacket(Protocol):
    """Pass the bytes between `START` and `STOP` to `handle_packet`."""

    START: ClassVar[bytes] = b"("
    STOP: ClassVar[bytes] = b")"

    def __init__(self) -> None:
        self.packet = bytearray()
        self.in_packet = False
        self.transport: ReaderThread[Any] | None = None

    def connection_made(self, transport: "ReaderThread[Any]") -> None:
        """Keep `transport`."""
        self.transport = transport

    def connection_lost(self, exc: Exception | None) -> None:
        """Forget the transport and any partial packet, then raise `exc` if set."""
        self.transport = None
        self.in_packet = False
        self.packet.clear()
        super().connection_lost(exc)

    def data_received(self, data: bytes) -> None:
        """Pass each packet to `handle_packet` once its `STOP` arrives.

        Bytes outside `START` and `STOP` go to `handle_out_of_packet_data`,
        one at a time.
        """
        for i in range(len(data)):
            byte = data[i : i + 1]
            if byte == self.START:
                self.in_packet = True
            elif byte == self.STOP:
                self.in_packet = False
                self.handle_packet(bytes(self.packet))
                self.packet.clear()
            elif self.in_packet:
                self.packet.extend(byte)
            else:
                self.handle_out_of_packet_data(byte)

    def handle_packet(self, packet: bytes) -> None:
        """Process one packet, without its markers.

        Raises
        ------
        NotImplementedError
            Unless a subclass overrides it.
        """
        raise NotImplementedError("please implement functionality in handle_packet")

    def handle_out_of_packet_data(self, data: bytes) -> None:
        """Process one byte received outside a packet; it is dropped here."""


class LineReader(Packetizer):
    """Read and write lines of text, encoded with `ENCODING`."""

    TERMINATOR: ClassVar[bytes] = b"\r\n"
    ENCODING: ClassVar[str] = "utf-8"
    UNICODE_HANDLING: ClassVar[str] = "replace"

    def handle_packet(self, packet: bytearray) -> None:
        """Decode `packet` and pass it to `handle_line`."""
        self.handle_line(packet.decode(self.ENCODING, self.UNICODE_HANDLING))

    def handle_line(self, line: str) -> None:
        """Process one line, without its terminator.

        Raises
        ------
        NotImplementedError
            Unless a subclass overrides it.
        """
        raise NotImplementedError("please implement functionality in handle_line")

    def write_line(self, text: str) -> None:
        """Write `text`, encoded and followed by `TERMINATOR`, in one write.

        Raises
        ------
        RuntimeError
            If the protocol is not connected.
        """
        if self.transport is None:
            raise RuntimeError("not connected")
        encoded = text.encode(self.ENCODING, self.UNICODE_HANDLING)
        self.transport.write(encoded + self.TERMINATOR)


P = TypeVar("P", bound=Protocol)


class ReaderThread(threading.Thread, Generic[P]):
    """A daemon thread that reads `serial` and passes the data to a protocol.

    `protocol_factory` makes the protocol when the thread starts. As a context
    manager it starts the thread, returns the protocol and closes the port on
    exit.
    """

    def __init__(
        self, serial_instance: Serial, protocol_factory: Callable[[], P]
    ) -> None:
        super().__init__()
        self.daemon = True
        self.serial = serial_instance
        self.protocol_factory = protocol_factory
        self.alive = True
        self._lock = threading.Lock()
        self._connection_made = threading.Event()
        self.protocol: P | None = None

    def stop(self) -> None:
        """End the reader loop and wait up to 2 s for the thread; the port stays open.

        Once the thread has ended, a cancel it did not need is dropped, so the
        next read on the port waits as usual.
        """
        self.alive = False
        self.serial.cancel_read()
        self.join(2)
        if not self.is_alive():
            self.serial._discard_cancel_read()

    def run(self) -> None:
        """Pass the data read to the protocol until stopped, closed or failed."""
        protocol = self.protocol_factory()
        self.protocol = protocol
        try:
            protocol.connection_made(self)
        except Exception as e:
            self.alive = False
            # Set even when connection_lost raises, so connect() does not wait forever.
            try:
                protocol.connection_lost(e)
            finally:
                self._connection_made.set()
            return
        error: Exception | None = None
        self._connection_made.set()
        while self.alive and self.serial.is_open:
            try:
                data = self.serial.read(self.serial.in_waiting or 1)
            except SerialException as e:
                error = e
                break
            if data:
                try:
                    protocol.data_received(data)
                except Exception as e:
                    error = e
                    break
        self.alive = False
        protocol.connection_lost(error)
        self.protocol = None

    def write(self, data: "Buffer | str") -> int:
        """Write `data` holding the lock, so writes from several threads do not mix."""
        with self._lock:
            return self.serial.write(data)

    def close(self) -> None:
        """Stop the reader, then close the port once no write holds the lock."""
        self.stop()
        with self._lock:
            self.serial.close()

    def connect(self) -> tuple[Self, P]:
        """Wait for `connection_made` and return this thread and its protocol.

        Raises
        ------
        RuntimeError
            If the thread was stopped, or the connection ended before it was made.
        """
        if not self.alive:
            raise RuntimeError("already stopped")
        self._connection_made.wait()
        if not self.alive or self.protocol is None:
            raise RuntimeError("connection_lost already called")
        return (self, self.protocol)

    def __enter__(self) -> P:
        """Start the thread and return the protocol once connected.

        Raises
        ------
        RuntimeError
            If the connection ended before it was made.
        """
        self.start()
        self._connection_made.wait()
        if not self.alive or self.protocol is None:
            raise RuntimeError("connection_lost already called")
        return self.protocol

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: TracebackType | None,
    ) -> None:
        """Close the port."""
        self.close()
