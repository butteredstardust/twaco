declare namespace twx {
    interface INFOTABLE<T = any> {
        rows: InfoTableRows<T>;
        dataShape: any;
        length: number;
        AddRow(row: Partial<T>): void;
        getRow(index: number): T;
        getRowCount(): number;
        RemoveAllRows(): void;
        Clone(): INFOTABLE<T>;
        ToJSON(): any;
        [member: string]: any;
    }

    interface InfoTableRows<T> {
        length: number;
        [index: number]: T;
        toArray(): T[];
    }

    interface LOCATION {
        latitude: number;
        longitude: number;
        elevation: number;
        units?: string;
    }

    interface Logger {
        trace(message: string, ...args: any[]): void;
        debug(message: string, ...args: any[]): void;
        info(message: string, ...args: any[]): void;
        warn(message: string, ...args: any[]): void;
        error(message: string, ...args: any[]): void;
    }
}

declare const logger: twx.Logger;
