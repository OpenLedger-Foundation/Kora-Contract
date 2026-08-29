export interface ExternalSigner {
  signTransaction(xdr: string): Promise<string>;
}

export async function signWithExternalSigner(signer: ExternalSigner, xdr: string) {
  return signer.signTransaction(xdr);
}
