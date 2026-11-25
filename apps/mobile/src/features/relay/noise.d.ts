declare module 'noise-handshake' {
  type Bytes = Uint8Array<ArrayBuffer>;
  export default class Noise {
    constructor(pattern: 'IK', initiator: boolean, identity?: { publicKey: Bytes; secretKey: Bytes });
    s: { publicKey: Bytes; secretKey: Bytes };
    tx: Bytes;
    rx: Bytes;
    initialise(prologue: Bytes, remoteStatic: Bytes): void;
    send(payload?: Bytes): Bytes;
    recv(message: Bytes): Bytes;
  }
}

declare module 'noise-handshake/cipher.js' {
  type Bytes = Uint8Array<ArrayBuffer>;
  export default class Cipher {
    constructor(key: Bytes);
    nonce: number;
    encrypt(message: Bytes): Bytes;
    decrypt(message: Bytes): Bytes;
  }
}
