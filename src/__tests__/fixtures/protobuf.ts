/** Minimal protobuf encoder for test fixtures (builds known BLOBs to walk). */

export function encodeVarint(n: number): Buffer {
  const bytes: number[] = [];
  let v = n >>> 0;
  do {
    let b = v & 0x7f;
    v = v >>> 7;
    if (v > 0) b |= 0x80;
    bytes.push(b);
  } while (v > 0);
  return Buffer.from(bytes);
}

export function encodeTag(fieldNumber: number, wireType: number): Buffer {
  return encodeVarint((fieldNumber << 3) | wireType);
}

export function encodeLengthDelimited(fieldNumber: number, data: Buffer): Buffer {
  return Buffer.concat([encodeTag(fieldNumber, 2), encodeVarint(data.length), data]);
}

export function encodeString(fieldNumber: number, str: string): Buffer {
  return encodeLengthDelimited(fieldNumber, Buffer.from(str, "utf8"));
}

export function encodeMessage(fieldNumber: number, sub: Buffer): Buffer {
  return encodeLengthDelimited(fieldNumber, sub);
}

export function encodeBytes(fieldNumber: number, data: Buffer): Buffer {
  return encodeLengthDelimited(fieldNumber, data);
}
