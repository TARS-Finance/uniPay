import { UsersRepository } from "@stacker/db";

export class UsersService {
  constructor(private readonly usersRepository: UsersRepository) {}

  async findByInitiaAddress(initiaAddress: string) {
    return this.usersRepository.findByInitiaAddress(initiaAddress);
  }

  async register(initiaAddress: string) {
    const existingUser = await this.findByInitiaAddress(initiaAddress);

    if (existingUser) {
      return existingUser;
    }

    return this.usersRepository.create(initiaAddress);
  }
}
